//! `arclain-cli extract ARCHIVE DESTINATION [ENTRY...] [--collision POLICY]`

use std::path::PathBuf;

use arclain_app::operations::{CollisionPolicy, ExtractRequest};
use arclain_app::ArclainApp;
use clap::{Args, ValueEnum};

use crate::output::{exit_code, exit_code_for, print_error, print_json_line, MutationOutcome};

/// Mirrors `arclain_app::operations::CollisionPolicy`'s own spelling
/// (`snake_case`, matching its `serde` rename) so `--collision skip`
/// reads the same on the command line as it does in this crate's own
/// `--json` output.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum CollisionPolicyArg {
    Ask,
    Overwrite,
    Rename,
    Skip,
}

impl CollisionPolicyArg {
    fn to_facade(self) -> CollisionPolicy {
        match self {
            Self::Ask => CollisionPolicy::Ask,
            Self::Overwrite => CollisionPolicy::Overwrite,
            Self::Rename => CollisionPolicy::Rename,
            Self::Skip => CollisionPolicy::Skip,
        }
    }
}

#[derive(Debug, Args)]
pub struct ExtractArgs {
    /// Path to the archive file to extract from.
    pub archive: PathBuf,
    /// Directory to extract into (created if it does not exist).
    pub destination: PathBuf,
    /// Archive-relative paths of the entries to extract (a directory
    /// path extracts every file beneath it). Omit entirely to extract
    /// the whole archive.
    pub entries: Vec<String>,
    /// How to handle a destination file that already exists. `ask`
    /// prompts interactively (refused with exit code 3 outside a real
    /// terminal); the other three apply unconditionally.
    #[arg(long, value_enum, default_value_t = CollisionPolicyArg::Ask)]
    pub collision: CollisionPolicyArg,
}

/// Opens `args.archive`, resolves `args.entries` to their current
/// `EntryId`s (empty means the whole archive), starts extraction, drives
/// it to a terminal state, then closes the session. Returns the process
/// exit code.
pub async fn run(app: &ArclainApp, args: &ExtractArgs, ctx: &super::Invocation) -> i32 {
    let interactive = crate::events::std_interactive();
    let snapshot = match super::open_archive_and_wait(
        app,
        &args.archive,
        &interactive,
        &ctx.cancel,
        ctx.budget,
    )
    .await
    {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };

    let entry_ids = if args.entries.is_empty() {
        Vec::new()
    } else {
        match super::resolve_entry_ids(app, snapshot.session_id, &args.entries).await {
            Ok(ids) => ids,
            Err(error) => {
                let code = exit_code_for(&error.kind);
                print_error(&error);
                let _ = app.close_archive(snapshot.session_id).await;
                return code;
            }
        }
    };

    let destination = match super::absolutize(&args.destination) {
        Ok(destination) => destination,
        Err(code) => {
            let _ = app.close_archive(snapshot.session_id).await;
            return code;
        }
    };

    // Subscribed before `start_extract` -- matches
    // `super::open_archive_and_wait`'s own established convention (see
    // its doc comment): subscribing after the call could race the
    // operation's own `Accepted` event.
    let mut events = app.subscribe_operations();
    let operation_id = match app
        .start_extract(ExtractRequest {
            session_id: snapshot.session_id,
            entry_ids,
            destination,
            collision_policy: args.collision.to_facade(),
        })
        .await
    {
        Ok(operation_id) => operation_id,
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            let _ = app.close_archive(snapshot.session_id).await;
            return code;
        }
    };

    let mut last_message = super::LastProgressMessage::default();
    let result = crate::events::drive_operation(
        crate::events::OperationWait {
            app,
            events: &mut events,
            operation_id,
            interactive: &interactive,
            cancel: &ctx.cancel,
            budget: ctx.budget,
        },
        ctx.json,
        |event| last_message.observe(event),
    )
    .await;

    // Best-effort: a successfully completed (or already-reported-failed)
    // extraction should not have its own exit code overridden by a
    // failure to close the now-finished session.
    let _ = app.close_archive(snapshot.session_id).await;

    match result {
        Ok(_) => {
            let summary = last_message.into_inner();
            if ctx.json {
                print_json_line(&MutationOutcome::completed(summary));
            } else {
                match &summary {
                    Some(summary) => println!("extraction complete: {summary}"),
                    None => println!("extraction complete"),
                }
            }
            exit_code::SUCCESS
        }
        Err(code) => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collision_policy_arg_round_trips_to_the_facade_spelling() {
        assert_eq!(
            serde_json::to_value(CollisionPolicyArg::Ask.to_facade()).unwrap(),
            serde_json::json!("ask")
        );
        assert_eq!(
            serde_json::to_value(CollisionPolicyArg::Overwrite.to_facade()).unwrap(),
            serde_json::json!("overwrite")
        );
        assert_eq!(
            serde_json::to_value(CollisionPolicyArg::Rename.to_facade()).unwrap(),
            serde_json::json!("rename")
        );
        assert_eq!(
            serde_json::to_value(CollisionPolicyArg::Skip.to_facade()).unwrap(),
            serde_json::json!("skip")
        );
    }
}
