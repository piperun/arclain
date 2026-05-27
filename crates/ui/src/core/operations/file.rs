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
    entries: &[FileEntry],
    status_info: &mut status_bar::StatusBarInfo,
    ui_entries: &mut Vec<FileEntry>,
) {
    // Build full paths using current navigation prefix; skip folders for delete
    let (full_paths, archive_opt) = {
        let st = state.lock();
        let tab = st.signals.tabs.get().active().clone();
        let prefix = tab.navigation.get().current_path.clone();
        // Selection lives in a path-keyed HashSet now (see FileEntry).
        let selection = tab.browser_view_state.get().selection;
        let fulls: Vec<String> = entries
            .iter()
            .filter(|e| selection.contains(&e.path) && !e.is_folder)
            .map(|e| {
                if prefix.is_empty() {
                    e.name.clone()
                } else {
                    format!("{}/{}", prefix, e.name)
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
                    ui_entries,
                );
            }
        }
    }
}
