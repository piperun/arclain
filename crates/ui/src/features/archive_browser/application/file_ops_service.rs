//! File operations application service

use crate::core::operations;
use crate::core::utils::convert_to_file_entry;
use crate::features::archive_operations::ArchiveOperationsState;
use crate::shared::models::file_entry::FileEntry;
use crate::shared::SharedState;

pub struct FileOpsService;

impl FileOpsService {
    pub fn delete_file(&self, shared: &SharedState, file: &str) {
        let archive_path = shared.signals().archive_path.get();
        if let Some(archive) = archive_path {
            let state = shared.app_state.lock();
            match state.backend_selector.select(&archive) {
                Ok(backend) => {
                    drop(state);
                    match backend.delete_files(&archive, &[file.to_string()]) {
                        Ok(()) => {
                            tracing::info!("Deleted file from archive: {}", file);
                            shared.signals().status_bar.update(|s| {
                                s.message = format!("Deleted: {}", file);
                            });
                            // Refresh entries after deletion
                            if let Ok(info) = backend.list(&archive, None) {
                                shared
                                    .signals()
                                    .entries
                                    .set(std::sync::Arc::new(info.entries));
                                // Update browser view state entries via signals
                                let entries = shared
                                    .signals()
                                    .entries
                                    .get()
                                    .iter()
                                    .map(convert_to_file_entry)
                                    .collect::<Vec<_>>();

                                shared.signals().browser_view_state.update(|s| {
                                    s.view_entries = entries;
                                });
                            }
                        }
                        Err(e) => {
                            let msg = format!("Failed to delete file: {}", e);
                            tracing::error!("{}", msg);
                            shared.signals().status_bar.update(|s| {
                                s.message = msg;
                            });
                        }
                    }
                }
                Err(e) => {
                    let msg = format!("No backend for archive: {}", e);
                    tracing::error!("{}", msg);
                    shared.signals().status_bar.update(|s| {
                        s.message = msg;
                    });
                }
            }
        } else {
            shared.signals().status_bar.update(|s| {
                s.message = "No archive loaded".to_string();
            });
        }
    }

    pub fn extract(
        &self,
        shared: &SharedState,
        ops_state: &mut ArchiveOperationsState,
        file: &str,
    ) {
        let entries = vec![FileEntry {
            name: file.to_string(),
            path: file.to_string(),
            selected: true,
            size: String::new(),
            compressed: String::new(),
            ratio: String::new(),
            modified: String::new(),
            crc32: String::new(),
            encrypted: false,
            is_folder: false,
        }];

        // We still need status_bar for the extraction call, but we can get it from signal
        // Wait, operations::extraction::extract_selected takes &mut StatusBarInfo.
        // We'll have to use signals.status_bar.update(...) inside or wrap the call.
        // Since extraction logic is deep, I'll temporarily pass a local mut and then write back
        // to signal, OR better, I'll update operations::extraction in Phase 4.
        // For now, I'll use a local Default and write it back.

        let mut temp_status = shared.signals().status_bar.get();
        let mut dialog = shared.signals().extraction_dialog.get();

        operations::extraction::extract_selected(
            &shared.app_state,
            &entries,
            &mut dialog,
            &mut ops_state.extraction_rx,
            &mut ops_state.extraction_child,
            &mut ops_state.extraction_minimized,
            &mut ops_state.extraction_started,
            &mut temp_status,
        );
        shared.signals().status_bar.set(temp_status);
        shared.signals().extraction_dialog.set(dialog);
    }

    pub fn edit_file(&self, shared: &SharedState, file: &str) {
        shared.signals().file_edit_dialog.update(|d| {
            d.show = true;
            d.full_path_in_archive = file.to_string();
            d.name_input = file.to_string();
        });

        if let Some(archive) = shared.signals().archive_path.get() {
            let state = shared.app_state.lock();
            match state.read_text_file(&archive, file) {
                Ok(content) => {
                    shared.signals().file_edit_dialog.update(|d| {
                        d.content = content.clone();
                        d.original_content = content;
                    });
                }
                Err(e) => {
                    let msg = format!("Failed to read file: {}", e);
                    crate::core::utils::log_failure("FileEdit", &msg);
                    shared.signals().status_bar.update(|s| {
                        s.message = msg;
                    });
                }
            }
        }
    }

    pub fn copy_path(
        &self,
        egui_ctx: &egui::Context,
        signals: &crate::core::signals::AppSignals,
        file: &str,
    ) {
        let nav = signals.navigation.get();
        let full_path = if nav.current_path.is_empty() {
            file.to_string()
        } else {
            format!("{}/{}", nav.current_path, file)
        };
        egui_ctx.copy_text(full_path);
    }
}
