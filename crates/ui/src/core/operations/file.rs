use crate::core::tabs::view_state::RevisionedSelection;
use crate::core::AppState;
use crate::shared::components::status_bar;
use crate::shared::models::file_entry::FileEntry;
use parking_lot::Mutex;
use std::sync::Arc;

/// Add files to the current archive
pub fn add_files(state: &Arc<Mutex<AppState>>, status_info: &mut status_bar::StatusBarInfo) {
    let archive_path = {
        let st = state.lock();
        st.signals.tabs.get().active().archive_path.get()
    };

    if let Some(archive) = archive_path {
        if let Some(files) = rfd::FileDialog::new().pick_files() {
            let st = state.lock();
            match st.add_files_to_archive(&archive, files) {
                Ok(()) => {
                    status_info.message = "Files added successfully".to_string();
                }
                Err(e) => {
                    status_info.message = format!("Add files failed: {}", e);
                }
            }
        }
    }
}

/// Delete selected files from the archive
///
/// Post 2026-05-20 Tier 2 (item 6) audit: dropped the `archive_info`
/// mutable parameter — the per-tab `Computed<ArchiveInfo>` re-derives
/// from the refreshed `entries` signal after `list_archive` runs.
pub fn delete_selected(
    state: &Arc<Mutex<AppState>>,
    selected_paths: &[String],
    status_info: &mut status_bar::StatusBarInfo,
) {
    // Build full paths using the current navigation prefix. The caller derives
    // this explicit selection from the immutable browser snapshot and current
    // search on the delete click, so hidden search results cannot leak into a
    // destructive operation.
    let (full_paths, archive_opt) = {
        let st = state.lock();
        let tab = st.signals.tabs.get().active().clone();
        let prefix = tab.navigation.get().current_path.clone();
        let fulls: Vec<String> = selected_paths
            .iter()
            .map(|path| {
                if prefix.is_empty() {
                    path.clone()
                } else {
                    format!("{prefix}/{path}")
                }
            })
            .collect();
        (fulls, tab.archive_path.get())
    };

    if full_paths.is_empty() {
        status_info.message = "No files selected".to_string();
        return;
    }

    if let Some(archive) = archive_opt {
        let res = { state.lock().delete_files(&archive, &full_paths) };
        if let Err(e) = res {
            status_info.message = format!("Delete failed: {}", e);
            return;
        }
        // Refresh listing
        let mut st = state.lock();
        let active_id = st.signals.tabs.get().active_id();
        if let Some(a) = st.signals.tabs.get().active().archive_path.get() {
            if st.list_archive(&a, active_id).is_ok() {
                drop(st);

                // We need to reload archive data - call the archive_operations module
                use crate::core::operations::archive;
                archive::load_archive_data(
                    state,
                    &mut Default::default(), // password_dialog placeholder
                    &mut None,               // pending_archive_path placeholder
                    status_info,
                );
            }
        }
    }
}

/// Derive the exact file paths affected by a toolbar delete action.
///
/// Selection may intentionally outlive a search filter, but destructive
/// toolbar actions apply only to selected rows visible under the current
/// search. Folder rows retain the existing non-deletable behavior.
pub(crate) fn selected_file_paths_for_search(
    entries: &[FileEntry],
    selection: &RevisionedSelection,
    search: &str,
) -> Vec<String> {
    let filter = search.trim().to_lowercase();
    entries
        .iter()
        .filter(|entry| {
            !entry.is_folder
                && selection.contains(&entry.path)
                && (filter.is_empty() || entry.name.to_lowercase().contains(&filter))
        })
        .map(|entry| entry.path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_folder: bool) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: name.to_string(),
            size: "0 B".to_string(),
            compressed: "0 B".to_string(),
            ratio: "0%".to_string(),
            modified: String::new(),
            crc32: String::new(),
            encrypted: false,
            is_folder,
        }
    }

    #[test]
    fn filtered_delete_emits_only_visible_selected_file_paths() {
        let entries = vec![
            entry("visible.txt", false),
            entry("hidden.txt", false),
            entry("visible-folder", true),
        ];
        let mut selection = RevisionedSelection::default();
        selection.extend([
            "visible.txt".to_string(),
            "hidden.txt".to_string(),
            "visible-folder".to_string(),
        ]);

        let paths = selected_file_paths_for_search(&entries, &selection, " visible ");

        assert_eq!(paths, vec!["visible.txt"]);
        assert!(selection.contains("hidden.txt"));
    }

    #[test]
    fn unfiltered_delete_emits_every_selected_file_path() {
        let entries = vec![
            entry("first.txt", false),
            entry("second.txt", false),
            entry("folder", true),
        ];
        let mut selection = RevisionedSelection::default();
        selection.extend([
            "first.txt".to_string(),
            "second.txt".to_string(),
            "folder".to_string(),
        ]);

        let paths = selected_file_paths_for_search(&entries, &selection, "  ");

        assert_eq!(paths, vec!["first.txt", "second.txt"]);
    }
}
