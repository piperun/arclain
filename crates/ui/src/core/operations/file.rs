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
                && selection.contains(&entry.archive_path)
                && (filter.is_empty() || entry.name.to_lowercase().contains(&filter))
        })
        .map(|entry| entry.archive_path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_folder: bool) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: name.to_string(),
            archive_path: name.to_string(),
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
        let mut entries = vec![
            entry("visible.txt", false),
            entry("hidden.txt", false),
            entry("visible-folder", true),
        ];
        entries[0].archive_path = "A/visible.txt".to_string();
        entries[1].archive_path = "A/hidden.txt".to_string();
        entries[2].archive_path = "A/visible-folder".to_string();
        let mut selection = RevisionedSelection::default();
        selection.extend([
            "A/visible.txt".to_string(),
            "A/hidden.txt".to_string(),
            "A/visible-folder".to_string(),
        ]);

        let paths = selected_file_paths_for_search(&entries, &selection, " visible ");

        assert_eq!(paths, vec!["A/visible.txt"]);
        assert!(selection.contains("A/hidden.txt"));
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
