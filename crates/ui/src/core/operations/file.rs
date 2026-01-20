use crate::core::operations::archive::ArchiveInfo;
use crate::core::AppState;
use crate::shared::components::status_bar;
use crate::shared::models::file_entry::FileEntry;
use parking_lot::Mutex;
use std::sync::Arc;

/// Add files to the current archive
pub fn add_files(state: &Arc<Mutex<AppState>>, status_info: &mut status_bar::StatusBarInfo) {
    let archive_path = {
        let st = state.lock();
        st.signals.archive_path.get() // Use signal
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
pub fn delete_selected(
    state: &Arc<Mutex<AppState>>,
    entries: &[FileEntry],
    status_info: &mut status_bar::StatusBarInfo,
    ui_entries: &mut Vec<FileEntry>,
    archive_info: &mut ArchiveInfo,
) {
    // Build full paths using current navigation prefix; skip folders for delete
    let (full_paths, archive_opt) = {
        let st = state.lock();
        let prefix = st.signals.navigation.get().current_path.clone();
        let fulls: Vec<String> = entries
            .iter()
            .filter(|e| e.selected && !e.is_folder)
            .map(|e| {
                if prefix.is_empty() {
                    e.name.clone()
                } else {
                    format!("{}/{}", prefix, e.name)
                }
            })
            .collect();
        (fulls, st.signals.archive_path.get()) // Use signal
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
        if let Some(a) = st.signals.archive_path.get() {
            if let Ok(entries) = st.list_archive(&a) {
                let current_archive = st.signals.archive_path.get();
                drop(st);

                // We need to reload archive data - call the archive_operations module
                use crate::core::operations::archive;
                archive::load_archive_data(
                    state,
                    entries,
                    current_archive,
                    &mut Default::default(), // password_dialog placeholder
                    &mut None,               // pending_archive_path placeholder
                    status_info,
                    ui_entries,
                    archive_info,
                );
            }
        }
    }
}
