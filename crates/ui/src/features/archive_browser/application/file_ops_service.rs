//! File operations application service

use crate::core::operations;
use crate::core::utils::convert_to_file_entry;
use crate::features::archive_operations::ArchiveOperationsState;
use crate::shared::SharedState;

pub struct FileOpsService;

impl FileOpsService {
    pub fn delete_file(&self, shared: &SharedState, file: &str) {
        let tab = shared.signals().tabs.get().active().clone();
        let archive_path = tab.archive_path.get();
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
                                tab.entries
                                    .set(std::sync::Arc::new(info.entries));
                                // Update browser view state entries via signals
                                let entries = tab
                                    .entries
                                    .get()
                                    .iter()
                                    .map(convert_to_file_entry)
                                    .collect::<Vec<_>>();

                                tab.browser_view_state.update(|s| {
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
        // Per-row extract: we have the file's basename directly and
        // bypass selection entirely. extract_selected now takes a
        // list of names so we just hand it `[file]` — no need to
        // synthesize a FileEntry or mutate the user's selection.
        let selected_names = vec![file.to_string()];

        // extraction_dialog is per-tab now (post 2026-05-20 B3 reframed
        // slice 2). The extract call here originates from a row in the
        // active tab's archive list, so the dialog lives on the active
        // tab.
        let active_tab = shared.signals().tabs.get().active().clone();
        let mut temp_status = shared.signals().status_bar.get();
        let mut dialog = active_tab.extraction_dialog().get();

        operations::extraction::extract_selected(
            &shared.app_state,
            &selected_names,
            &mut dialog,
            &mut ops_state.extraction_rx,
            &mut ops_state.extraction_child,
            &mut ops_state.extraction_minimized,
            &mut ops_state.extraction_started,
            &mut ops_state.extraction_op_guard,
            &mut ops_state.extraction_origin_tab,
            &mut temp_status,
        );
        shared.signals().status_bar.set(temp_status);
        active_tab.extraction_dialog().set(dialog);
    }

    pub fn edit_file(&self, shared: &SharedState, file: &str) {
        shared.signals().tabs.get().active().file_edit_dialog.update(|d| {
            d.show = true;
            d.full_path_in_archive = file.to_string();
            d.name_input = file.to_string();
        });

        if let Some(archive) = shared.signals().tabs.get().active().archive_path.get() {
            let state = shared.app_state.lock();
            match state.read_text_file(&archive, file) {
                Ok(content) => {
                    shared.signals().tabs.get().active().file_edit_dialog.update(|d| {
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
        let nav = signals.tabs.get().active().navigation.get();
        let full_path = if nav.current_path.is_empty() {
            file.to_string()
        } else {
            format!("{}/{}", nav.current_path, file)
        };
        egui_ctx.copy_text(full_path);
    }
}
