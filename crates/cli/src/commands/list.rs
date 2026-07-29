//! `arclain-cli list ARCHIVE [ARCHIVE_PATH] [--offset N] [--limit N] [--json]`

use std::path::PathBuf;

use arclain_app::archive::{
    ArchivePath, EntryKind, EntryPage, EntrySortKey, ListEntriesRequest, SortDirection,
};
use arclain_app::ArclainApp;
use clap::Args;

use crate::output::{exit_code, exit_code_for, print_error, print_json};

/// Default page size when `--limit` is omitted -- matches the
/// "give me everything reasonable" convention already used by several of
/// `arclain_app`'s own fixture-driven tests (e.g.
/// `crates/app/tests/archive_mutation.rs`).
const DEFAULT_LIMIT: u32 = 1000;

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Path to the archive file to list.
    pub archive: PathBuf,

    /// Directory within the archive to list (defaults to the archive
    /// root). Forward- or backslash-separated; must not be absolute or
    /// contain a `..` segment.
    pub archive_path: Option<String>,

    /// How many matching entries to skip before the first one returned.
    #[arg(long, default_value_t = 0)]
    pub offset: u64,

    /// The maximum number of entries to return.
    #[arg(long, default_value_t = DEFAULT_LIMIT)]
    pub limit: u32,
}

/// Opens `args.archive`, lists one page of `args.archive_path` (or the
/// archive root), prints it, then closes the session. Returns the
/// process exit code.
pub async fn run(app: &ArclainApp, args: &ListArgs, json: bool) -> i32 {
    // Validated before ever opening the archive: a malformed in-archive
    // path is a purely local input error, cheaper and clearer to reject
    // up front than after an archive-open round trip.
    let directory = match ArchivePath::parse(args.archive_path.clone().unwrap_or_default()) {
        Ok(directory) => directory,
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            return code;
        }
    };

    let interactive = crate::events::std_interactive();
    let snapshot = match super::open_archive_and_wait(app, &args.archive, &interactive).await {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };

    let request = ListEntriesRequest {
        directory,
        sort_key: EntrySortKey::Name,
        sort_direction: SortDirection::Ascending,
        name_filter: None,
        offset: args.offset,
        limit: args.limit,
    };

    let page = match app.list_entries(snapshot.session_id, request).await {
        Ok(page) => page,
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            let _ = app.close_archive(snapshot.session_id).await;
            return code;
        }
    };

    if json {
        print_json(&page);
    } else {
        print_page_human(&page);
    }

    let _ = app.close_archive(snapshot.session_id).await;
    exit_code::SUCCESS
}

fn print_page_human(page: &EntryPage) {
    for entry in &page.entries {
        let marker = match entry.kind {
            EntryKind::Directory => 'd',
            EntryKind::File => 'f',
            EntryKind::Symlink => 'l',
        };
        println!("{marker}  {:>12}  {}", entry.uncompressed_size, entry.name);
    }
    println!("-- {} of {} entries --", page.entries.len(), page.total);
}
