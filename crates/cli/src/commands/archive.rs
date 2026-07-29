//! `arclain-cli archive add ARCHIVE SOURCE...` / `arclain-cli archive delete ARCHIVE ENTRY...`

use std::path::PathBuf;

use arclain_app::archive::ArchivePath;
use arclain_app::event::OperationState;
use arclain_app::ids::ArchiveSessionId;
use arclain_app::operations::ArchiveMutationRequest;
use arclain_app::ArclainApp;
use clap::{Args, Subcommand};

use crate::output::{
    exit_code_for, print_error, print_json_line, print_plain_error, MutationOutcome,
};

#[derive(Debug, Subcommand)]
pub enum ArchiveCommand {
    /// Add real filesystem files into an archive's root.
    Add(AddArgs),
    /// Delete entries from an archive (a directory entry deletes every
    /// file beneath it).
    Delete(DeleteArgs),
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// Path to the archive file to mutate.
    pub archive: PathBuf,
    /// Real filesystem files to add. Every backend in this workspace
    /// adds new members at the archive root, keyed by each source
    /// file's own basename -- there is no way to add into a specific
    /// in-archive subfolder yet.
    #[arg(required = true)]
    pub sources: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// Path to the archive file to mutate.
    pub archive: PathBuf,
    /// Archive-relative paths of the entries to delete (a directory
    /// path deletes every file beneath it).
    #[arg(required = true)]
    pub entries: Vec<String>,
}

pub async fn dispatch(app: &ArclainApp, command: &ArchiveCommand, json: bool) -> i32 {
    match command {
        ArchiveCommand::Add(args) => run_add(app, args, json).await,
        ArchiveCommand::Delete(args) => run_delete(app, args, json).await,
    }
}

async fn run_add(app: &ArclainApp, args: &AddArgs, json: bool) -> i32 {
    for source in &args.sources {
        if !source.is_file() {
            print_plain_error(&format!("source not found: {}", source.display()));
            return crate::output::exit_code::UNSUPPORTED_INPUT;
        }
    }

    let interactive = crate::events::std_interactive();
    let snapshot = match super::open_archive_and_wait(app, &args.archive, &interactive).await {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };

    let mut source_paths = Vec::with_capacity(args.sources.len());
    for source in &args.sources {
        match super::absolutize(source) {
            Ok(path) => source_paths.push(path),
            Err(code) => {
                let _ = app.close_archive(snapshot.session_id).await;
                return code;
            }
        }
    }

    let request = ArchiveMutationRequest::AddFiles {
        session_id: snapshot.session_id,
        expected_revision: snapshot.revision,
        destination: ArchivePath::root(),
        source_paths,
    };
    run_mutation(app, snapshot.session_id, request, json).await
}

async fn run_delete(app: &ArclainApp, args: &DeleteArgs, json: bool) -> i32 {
    let interactive = crate::events::std_interactive();
    let snapshot = match super::open_archive_and_wait(app, &args.archive, &interactive).await {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };

    let entry_ids = match super::resolve_entry_ids(app, snapshot.session_id, &args.entries).await {
        Ok(ids) => ids,
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            let _ = app.close_archive(snapshot.session_id).await;
            return code;
        }
    };

    let request = ArchiveMutationRequest::DeleteEntries {
        session_id: snapshot.session_id,
        expected_revision: snapshot.revision,
        entry_ids,
    };
    run_mutation(app, snapshot.session_id, request, json).await
}

/// Shared by `add`/`delete`: submits `request` (`expected_revision`
/// already read from a fresh snapshot by the caller), drives it to a
/// terminal state, captures a `SnapshotChanged` event's new revision
/// along the way for the human-mode summary, and always closes
/// `session_id` (best-effort) before returning.
async fn run_mutation(
    app: &ArclainApp,
    session_id: ArchiveSessionId,
    request: ArchiveMutationRequest,
    json: bool,
) -> i32 {
    let mut events = app.subscribe_operations();
    let operation_id = match app.start_archive_mutation(request).await {
        Ok(operation_id) => operation_id,
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            let _ = app.close_archive(session_id).await;
            return code;
        }
    };

    let interactive = crate::events::std_interactive();
    let mut cancel = crate::events::CancelTrigger::CtrlC;
    let mut new_revision: Option<u64> = None;
    let result = crate::events::drive_operation(
        app,
        &mut events,
        operation_id,
        json,
        &interactive,
        &mut cancel,
        |event| {
            if let OperationState::SnapshotChanged { revision, .. } = &event.state {
                new_revision = Some(*revision);
            }
        },
    )
    .await;

    let _ = app.close_archive(session_id).await;

    match result {
        Ok(_) => {
            // Archive mutations report a `SnapshotChanged` revision, not
            // a per-file tally message -- no `LastProgressMessage` to
            // capture here (`run_archive_mutation` never emits
            // `Progress` at all).
            if json {
                print_json_line(&MutationOutcome::completed_with_revision(new_revision));
            } else if let Some(revision) = new_revision {
                println!("archive updated to revision {revision}");
            } else {
                println!("no changes were needed");
            }
            crate::output::exit_code::SUCCESS
        }
        Err(code) => code,
    }
}
