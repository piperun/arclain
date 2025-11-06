use crate::app::state::AppState;
use crate::features::{dialogs, file_list, status_bar};
use arclain_core::sevenzip::ProgressUpdate;
use parking_lot::Mutex;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

/// Extract selected files from the archive
pub fn extract_selected(
    state: &Arc<Mutex<AppState>>,
    entries: &[file_list::FileEntry],
    extraction_child: &Option<std::process::Child>,
    extraction_dialog: &mut dialogs::ExtractionProgressDialog,
    extraction_rx: &mut Option<Receiver<ProgressUpdate>>,
    extraction_child_mut: &mut Option<std::process::Child>,
    extraction_minimized: &mut bool,
    extraction_started: &mut Option<Instant>,
    status_info: &mut status_bar::StatusBarInfo,
) {
    if extraction_child.is_some() {
        status_info.message = "Another extraction is already running".to_string();
        return;
    }

    let st = state.lock();
    if let Some(archive) = &st.current_archive {
        let selected_files: Vec<String> = entries
            .iter()
            .filter(|e| e.selected)
            .map(|e| e.name.clone())
            .collect();

        if selected_files.is_empty() {
            status_info.message = "No files selected".to_string();
            return;
        }

        // Build full paths using navigation prefix
        let full_paths: Vec<String> = if !st.navigation.current_path.is_empty() {
            selected_files
                .iter()
                .map(|f| format!("{}/{}", st.navigation.current_path, f))
                .collect()
        } else {
            selected_files
        };

        let archive_clone = archive.clone();
        let backend = st.backend.clone();
        let auto_pw = st.cfg.auto_password_for(&st.last_entries);
        let pw_opt = st
            .current_password
            .as_deref()
            .or(auto_pw.as_deref())
            .map(|s| s.to_string());
        drop(st);

        if let Some(dest) = rfd::FileDialog::new().pick_folder() {
            match backend.spawn_extract_files_with_progress(
                &archive_clone,
                &dest,
                &full_paths,
                pw_opt.as_deref(),
            ) {
                Ok(handle) => {
                    *extraction_dialog = dialogs::ExtractionProgressDialog::default();
                    extraction_dialog.show = true;
                    extraction_dialog.title = format!("Extracting to {}", dest.display());
                    extraction_dialog.file_action = "Extracting selected files".to_string();
                    #[cfg(target_os = "windows")]
                    {
                        extraction_dialog.can_pause = true;
                    }
                    *extraction_rx = Some(handle.rx);
                    *extraction_child_mut = Some(handle.child);
                    *extraction_minimized = false;
                    *extraction_started = Some(Instant::now());
                    status_info.message = "Extraction started".to_string();
                }
                Err(e) => {
                    status_info.message = format!("Failed to start extraction: {}", e);
                }
            }
        }
    }
}

/// Extract all files from the archive
pub fn extract_all(
    state: &Arc<Mutex<AppState>>,
    extraction_child: &Option<std::process::Child>,
    extraction_dialog: &mut dialogs::ExtractionProgressDialog,
    extraction_rx: &mut Option<Receiver<ProgressUpdate>>,
    extraction_child_mut: &mut Option<std::process::Child>,
    extraction_minimized: &mut bool,
    extraction_started: &mut Option<Instant>,
    status_info: &mut status_bar::StatusBarInfo,
) {
    if extraction_child.is_some() {
        status_info.message = "Another extraction is already running".to_string();
        return;
    }

    let st = state.lock();
    if let Some(archive) = &st.current_archive {
        let archive_clone = archive.clone();
        let backend = st.backend.clone();
        let auto_pw = st.cfg.auto_password_for(&st.last_entries);
        let pw_opt = st
            .current_password
            .as_deref()
            .or(auto_pw.as_deref())
            .map(|s| s.to_string());
        drop(st);

        if let Some(dest) = rfd::FileDialog::new().pick_folder() {
            match backend.spawn_extract_all_with_progress(&archive_clone, &dest, pw_opt.as_deref())
            {
                Ok(handle) => {
                    *extraction_dialog = dialogs::ExtractionProgressDialog::default();
                    extraction_dialog.show = true;
                    extraction_dialog.title = format!("Extracting all to {}", dest.display());
                    extraction_dialog.file_action = "Extracting all files".to_string();
                    #[cfg(target_os = "windows")]
                    {
                        extraction_dialog.can_pause = true;
                    }
                    *extraction_rx = Some(handle.rx);
                    *extraction_child_mut = Some(handle.child);
                    *extraction_minimized = false;
                    *extraction_started = Some(Instant::now());
                    status_info.message = "Extraction started".to_string();
                }
                Err(e) => {
                    status_info.message = format!("Failed to start extraction: {}", e);
                }
            }
        }
    }
}
