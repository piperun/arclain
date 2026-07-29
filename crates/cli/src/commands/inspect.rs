//! `arclain-cli inspect ARCHIVE [--json]`

use std::path::PathBuf;

use arclain_app::archive::ArchiveSnapshot;
use arclain_app::ArclainApp;
use clap::Args;

use crate::output::{exit_code, print_json};

#[derive(Debug, Args)]
pub struct InspectArgs {
    /// Path to the archive file to inspect.
    pub archive: PathBuf,
}

/// Opens `args.archive`, prints its snapshot, then closes the session.
/// Returns the process exit code.
pub async fn run(app: &ArclainApp, args: &InspectArgs, json: bool) -> i32 {
    let interactive = crate::events::std_interactive();
    let snapshot = match super::open_archive_and_wait(app, &args.archive, &interactive).await {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };

    if json {
        print_json(&snapshot);
    } else {
        print_snapshot_human(&snapshot);
    }

    // Best-effort: the command already has everything it needs to report
    // success, so a failure to close is not this command's failure.
    let _ = app.close_archive(snapshot.session_id).await;
    exit_code::SUCCESS
}

fn print_snapshot_human(snapshot: &ArchiveSnapshot) {
    println!("source_path: {}", snapshot.source_path.display());
    println!("archive_type: {}", snapshot.archive_type);
    println!("entry_count: {}", snapshot.entry_count);
    println!(
        "total_uncompressed_size: {}",
        snapshot.total_uncompressed_size
    );
    if let Some(comment) = &snapshot.comment {
        println!("comment: {comment}");
    }
}
