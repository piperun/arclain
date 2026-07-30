//! TRANSITIONAL(4c): the pre-facade flat-listing projections the archive
//! browser's renderer still reads.
//!
//! Publishing a tab's visible rows *should* be one
//! `ArclainApp::list_entries` call for the directory the tab's
//! [`TabListing`] names, converted with
//! [`crate::core::utils::file_entry_from_dto`] -- and the tab now holds
//! exactly such pages, adopted by the bridge on every relist. What keeps
//! this module alive is *navigation responsiveness*: the renderer's rows
//! must update the instant the user navigates, and pages are only
//! fetched on relists today, so navigating scopes the tab's
//! whole-archive inventory (its `legacy_rows` projection of the
//! session's own rows) through the pre-facade filter instead. The
//! render-side migration adds the fetch-on-navigate (with the
//! loading/failure rendering `TabListing` already models) and deletes
//! this module whole.
//!
//! Every remaining `arclain_core` reference in the browser's navigation
//! path is therefore collected here rather than spread across the render
//! tree: this module is what the render-side migration deletes, whole.
//!
//! [`TabListing`]: crate::core::tabs::TabListing

use crate::core::signals::AppSignals;
use crate::core::tabs::{TabId, TabState};
use crate::shared::models::file_entry::FileEntry;
use std::sync::Arc;
use tracing::info;

/// Publish the active tab's worker-owned browser snapshot for its current path.
pub fn refresh_view_entries(signals: &AppSignals) {
    let tab = signals.tabs.get().active().clone();
    refresh_view_entries_for(&tab);
}

/// Refresh browser entries for a specific tab. Used when an archive is
/// loaded into a non-active tab (e.g. multi-file drop opens several
/// tabs but only the last is active; each must populate its own
/// browser snapshot so a future switch shows the file list immediately).
pub fn refresh_view_entries_for_tab(signals: &AppSignals, tab_id: TabId) {
    let Some(tab) = signals.tabs.get().get(tab_id).cloned() else {
        return;
    };
    refresh_view_entries_for(&tab);
}

fn refresh_view_entries_for(tab: &Arc<TabState>) {
    let all_entries = tab.inventory.get().legacy_rows();
    let current_path = tab.listing.get().current_path().to_string();

    info!(
        "refresh_view_entries: filtering to path='{}' (len={})",
        current_path,
        all_entries.len()
    );

    let entries = rows_in_directory(&all_entries, &current_path);

    info!(
        "refresh_view_entries: got {} entries for path '{}'",
        entries.len(),
        current_path
    );

    tab.browser_entries
        .update(|snapshot| snapshot.replace(entries));
}

/// TRANSITIONAL(4c): the display rows for `directory`, scoped out of a
/// whole-archive flat listing.
///
/// The facade equivalent is `ArclainApp::list_entries` for the same
/// directory, whose `EntryPage` rows convert through
/// [`crate::core::utils::file_entry_from_dto`]. Both produce the same rows
/// for the same directory -- the session's own entry index reproduces this
/// filter's folder synthesis and its recursive size/CRC aggregation -- but
/// they order ties differently: this one keys on the display-relative path
/// (so an uppercase name sorts before every lowercase one), the session's
/// on the lowercased name. Immaterial in practice, because the file list
/// re-sorts every row itself before rendering.
pub fn rows_in_directory(
    entries: &[arclain_core::ArchiveEntry],
    directory: &str,
) -> Vec<FileEntry> {
    let mut navigation = arclain_core::archive::NavigationState::new();
    navigation.set_current_path(directory);
    navigation
        .filter_entries_with_archive_paths(entries)
        .iter()
        .map(|visible| row_from_core_entry(&visible.entry, &visible.archive_path))
        .collect()
}

/// TRANSITIONAL(4c): every folder in a whole-archive flat listing, for the
/// tree panel's own projection.
///
/// The input is the inventory's legacy projection now, whose directory
/// rows already carry `is_dir` for every folder the session synthesized
/// -- this walk re-derives the same set the pre-facade way until the
/// tree panel reads DTO rows directly. Free function rather than a
/// method call on a throwaway `NavigationState` value: the walk never
/// reads that type's cursor at all.
pub fn all_folders(entries: &[arclain_core::ArchiveEntry]) -> Vec<String> {
    arclain_core::archive::NavigationState::new().get_all_folders(entries)
}

/// TRANSITIONAL(4c): one display row built from a pre-facade listing
/// entry, whose `path` is already relative to the folder on screen and
/// whose archive-root path is supplied separately.
///
/// [`crate::core::utils::file_entry_from_dto`] is the facade-typed
/// counterpart; keep the two in step until this one goes.
fn row_from_core_entry(entry: &arclain_core::ArchiveEntry, archive_path: &str) -> FileEntry {
    let ratio = if entry.size > 0 {
        format!("{}%", (entry.packed_size * 100 / entry.size))
    } else {
        "0%".to_string()
    };

    FileEntry {
        name: std::path::Path::new(&entry.path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| entry.path.clone()),
        path: entry.path.clone(),
        archive_path: archive_path.to_string(),
        size: crate::core::utils::format_size(entry.size),
        compressed: crate::core::utils::format_size(entry.packed_size),
        ratio,
        modified: entry.modified.clone().unwrap_or_default(),
        crc32: entry.crc32.clone().unwrap_or_default(),
        encrypted: entry.encrypted,
        is_folder: entry.is_dir,
    }
}
