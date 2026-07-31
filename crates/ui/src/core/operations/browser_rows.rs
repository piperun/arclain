//! Publishes the archive browser's display rows for the directory a tab
//! is browsing, out of the session's own listing rows.
//!
//! A tab holds its archive's whole entry tree as
//! [`ArchiveEntryDto`]s -- the [`TabInventory`] the bridge adopts from
//! `ArclainApp::list_all_entries` on every relist. Showing one folder is
//! then just taking the entries whose parent is the browsed directory
//! and converting each with
//! [`crate::core::utils::file_entry_from_dto`].
//!
//! # Why scoping the inventory is the session's own answer
//!
//! The inventory is fetched on *relists* (open, mutation, CRC backfill),
//! not on navigation, and that is what keeps navigation instant: clicking
//! a folder repaints from data already in memory instead of waiting on a
//! round trip.
//!
//! That only holds because scoping it agrees with asking. Listing one
//! directory returns the session index's direct children of it; this is
//! the same index's entries filtered to the same parent.
//! `crates/ui/tests/tab_archive_model_test.rs` hands the exact
//! `list_entries` request a tab's [`TabListing`] names to the session and
//! asserts row-for-row, field-for-field equality with what this produces,
//! at every level of a real archive -- so this is a *scoping* of the
//! session's answer, not a second listing pipeline reconstructing one.
//!
//! Whether these rows have been produced *at all* is not visible from the
//! rows: an archive nobody has listed yet publishes none, and so does an
//! empty directory. That distinction lives on [`TabListing`]'s status, and
//! the browser reads it from there rather than from a row count.
//!
//! [`ArchiveEntryDto`]: arclain_app::archive::ArchiveEntryDto
//! [`TabInventory`]: crate::core::tabs::TabInventory
//! [`TabListing`]: crate::core::tabs::TabListing

use crate::core::signals::AppSignals;
use crate::core::tabs::{TabId, TabState};
use crate::core::utils::file_entry_from_dto;
use crate::shared::models::file_entry::FileEntry;
use arclain_app::archive::{ArchiveEntryDto, ArchivePath, EntryKind};
use std::sync::Arc;
use tracing::info;

/// Republishes the active tab's browser rows for the folder it is
/// browsing.
pub fn publish_browsed_directory(signals: &AppSignals) {
    let tab = signals.tabs.get().active().clone();
    publish_for(&tab);
}

/// Republishes one specific tab's browser rows. Used when an archive is
/// loaded into a non-active tab (e.g. a multi-file drop opens several
/// tabs but only the last is active; each must publish its own rows so a
/// later switch shows the file list immediately).
pub fn publish_browsed_directory_for_tab(signals: &AppSignals, tab_id: TabId) {
    let Some(tab) = signals.tabs.get().get(tab_id).cloned() else {
        return;
    };
    publish_for(&tab);
}

fn publish_for(tab: &Arc<TabState>) {
    let inventory = tab.inventory.get();
    let directory = tab.listing.get().directory().clone();
    let rows = rows_in_directory(inventory.entries(), &directory);

    info!(
        "publish_browsed_directory: {} of {} entries are in '{}'",
        rows.len(),
        inventory.entry_count(),
        directory.as_str()
    );

    tab.browser_entries
        .update(|snapshot| snapshot.replace(rows));
}

/// The archive-relative directory holding `path`, empty for a
/// root-level entry.
fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(position) => &path[..position],
        None => "",
    }
}

/// The display rows for `directory`: every entry whose parent is exactly
/// `directory`, in the order the session listed them.
///
/// Not sorted here. The file list owns the sort the user picked and
/// re-sorts every row before rendering
/// (`BrowserProjectionCache::render_projection`), so imposing one here
/// would only be work thrown away.
pub fn rows_in_directory(entries: &[ArchiveEntryDto], directory: &ArchivePath) -> Vec<FileEntry> {
    entries
        .iter()
        .filter(|entry| parent_of(entry.path.as_str()) == directory.as_str())
        .map(file_entry_from_dto)
        .collect()
}

/// Every folder in the archive, for the tree panel's own projection.
///
/// The session's index synthesizes a `Directory` entry for every folder
/// an entry path implies, so the directory rows *are* the folder set --
/// there is nothing to re-derive from file paths. Sorted, because the
/// tree panel is the one consumer whose input is otherwise unordered
/// (its own child sort is by name, not by path) and a stable folder list
/// makes the projection deterministic.
pub fn folder_paths(entries: &[ArchiveEntryDto]) -> Vec<String> {
    let mut folders: Vec<String> = entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::Directory)
        .map(|entry| entry.path.as_str().to_string())
        .collect();
    folders.sort();
    folders
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_app::ids::EntryId;

    fn dto(id: u64, path: &str, kind: EntryKind) -> ArchiveEntryDto {
        ArchiveEntryDto {
            id: EntryId::from_raw(id),
            path: ArchivePath::parse(path.to_string()).unwrap(),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            kind,
            compressed_size: Some(4),
            uncompressed_size: 8,
            modified_at_unix_ms: None,
            encrypted: false,
            crc32: None,
        }
    }

    /// A depth-first inventory of a small archive, the shape
    /// `list_all_entries` answers with.
    fn inventory() -> Vec<ArchiveEntryDto> {
        vec![
            dto(1, "game", EntryKind::Directory),
            dto(2, "game/Game.exe", EntryKind::File),
            dto(3, "game/data", EntryKind::Directory),
            dto(4, "game/data/save.dat", EntryKind::File),
            dto(5, "readme.txt", EntryKind::File),
        ]
    }

    fn paths_in(directory: &str) -> Vec<String> {
        rows_in_directory(
            &inventory(),
            &ArchivePath::parse(directory.to_string()).unwrap(),
        )
        .into_iter()
        .map(|row| row.archive_path)
        .collect()
    }

    #[test]
    fn the_root_shows_its_own_entries_and_no_nested_ones() {
        assert_eq!(paths_in(""), ["game", "readme.txt"]);
    }

    #[test]
    fn a_directory_shows_its_direct_children_only() {
        assert_eq!(paths_in("game"), ["game/Game.exe", "game/data"]);
        assert_eq!(paths_in("game/data"), ["game/data/save.dat"]);
    }

    /// A prefix match alone would put `game/data/save.dat` in `game`,
    /// and a sibling directory whose name merely starts with the browsed
    /// one (`game2`) would leak into it as well.
    #[test]
    fn a_directory_does_not_capture_deeper_or_similarly_named_entries() {
        let mut entries = inventory();
        entries.push(dto(6, "game2", EntryKind::Directory));
        entries.push(dto(7, "game2/other.txt", EntryKind::File));

        let rows = rows_in_directory(&entries, &ArchivePath::parse("game".to_string()).unwrap());
        let paths: Vec<&str> = rows.iter().map(|row| row.archive_path.as_str()).collect();
        assert_eq!(paths, ["game/Game.exe", "game/data"]);
    }

    #[test]
    fn a_directory_nothing_lives_in_shows_nothing() {
        assert!(paths_in("game/data/save.dat").is_empty());
    }

    /// A row's display path is its name while `archive_path` stays
    /// archive-root-relative -- what keeps selection keyed on identity
    /// rather than on whatever folder is on screen.
    #[test]
    fn a_row_shows_its_name_and_keys_on_its_archive_path() {
        let rows = rows_in_directory(
            &inventory(),
            &ArchivePath::parse("game/data".to_string()).unwrap(),
        );
        assert_eq!(rows[0].path, "save.dat");
        assert_eq!(rows[0].archive_path, "game/data/save.dat");
    }

    /// The tree's folder set is exactly the index's directory rows: the
    /// session already synthesized `game/data` from the file path below
    /// it, so nothing here re-derives ancestors.
    #[test]
    fn the_folder_set_is_every_directory_row_sorted() {
        assert_eq!(folder_paths(&inventory()), ["game", "game/data"]);
    }

    #[test]
    fn an_empty_inventory_has_no_rows_and_no_folders() {
        assert!(rows_in_directory(&[], &ArchivePath::root()).is_empty());
        assert!(folder_paths(&[]).is_empty());
    }
}
