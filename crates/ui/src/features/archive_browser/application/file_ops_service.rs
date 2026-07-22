//! File operations application service

use crate::core::operations;
use crate::core::tabs::{OpGuard, TabState};
use crate::core::utils::convert_to_file_entry_with_archive_path;
use crate::features::archive_operations::ArchiveOperationsState;
use crate::features::file_editing::domain::types::FileEditLoadState;
use crate::shared::models::file_entry::FileEntry;
use crate::shared::SharedState;
use anyhow::{anyhow, Result};
use arclain_core::backends::BackendSelector;
use arclain_core::{ArchiveEntry, NavigationState};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Narrow I/O boundary used to verify that archive work is scheduled rather
/// than executed by the caller. Public only because integration tests compile
/// as a separate crate.
#[doc(hidden)]
pub trait ArchiveFileIo: Send + Sync {
    fn delete_and_list(&self, archive: &Path, paths: &[String]) -> Result<DeleteListResult>;
    fn read_text(&self, archive: &Path, path: &str) -> Result<String>;
}

#[doc(hidden)]
pub struct DeleteListResult {
    pub archive_entries: Arc<Vec<ArchiveEntry>>,
    pub browser_entries: Vec<FileEntry>,
}

struct BackendArchiveFileIo {
    backend_selector: BackendSelector,
    password: Option<String>,
    navigation: NavigationState,
}

impl BackendArchiveFileIo {
    fn capture(shared: &SharedState, origin: &TabState) -> Self {
        Self {
            backend_selector: shared.app_state.lock().backend_selector.clone(),
            password: origin.current_password.get(),
            navigation: origin.navigation.get(),
        }
    }
}

impl ArchiveFileIo for BackendArchiveFileIo {
    fn delete_and_list(&self, archive: &Path, paths: &[String]) -> Result<DeleteListResult> {
        let backend = self.backend_selector.select(archive)?;
        backend.delete_files(archive, paths)?;
        let archive_entries = backend.list(archive, self.password.as_deref())?.entries;
        let browser_entries = self
            .navigation
            .filter_entries_with_archive_paths(&archive_entries)
            .into_iter()
            .map(|item| convert_to_file_entry_with_archive_path(&item.entry, &item.archive_path))
            .collect();
        Ok(DeleteListResult {
            archive_entries: Arc::new(archive_entries),
            browser_entries,
        })
    }

    fn read_text(&self, archive: &Path, path: &str) -> Result<String> {
        let backend = self.backend_selector.select(archive)?;
        backend.read_text_file(archive, path, self.password.as_deref())
    }
}

pub struct FileOpsService;

impl FileOpsService {
    pub fn delete_files(&self, shared: &SharedState, origin: Arc<TabState>, paths: Vec<String>) {
        if paths.is_empty() {
            shared.signals().status_bar.update(|status| {
                status.message = "No files selected".to_string();
            });
            return;
        }

        let io = Arc::new(BackendArchiveFileIo::capture(shared, &origin));
        self.delete_files_with_io(shared, origin, paths, io);
    }

    #[doc(hidden)]
    pub fn delete_files_with_io(
        &self,
        shared: &SharedState,
        origin: Arc<TabState>,
        paths: Vec<String>,
        io: Arc<dyn ArchiveFileIo>,
    ) {
        let Some(archive) = origin.archive_path.get() else {
            shared.signals().status_bar.update(|status| {
                status.message = "No archive loaded".to_string();
            });
            return;
        };

        let runtime = shared.services.tokio_runtime.clone();
        let status_bar = shared.signals().status_bar.clone();
        let edit_lock = origin.archive_edit_lock.clone();
        let guard = OpGuard::new(&origin);
        let deleted_count = paths.len();

        runtime.spawn(async move {
            let _guard = guard;
            let worker_origin = origin.clone();
            let worker_archive = archive.clone();
            let worker_status_bar = status_bar.clone();
            let worker_result = tokio::task::spawn_blocking(move || {
                let _edit_guard = edit_lock.lock();
                let result = io.delete_and_list(&worker_archive, &paths);

                // Publication is part of the serialized edit. Releasing the
                // lock before swapping the snapshot lets a later edit finish
                // and publish first, after which this older result can
                // overwrite it.
                if worker_origin.archive_path.get().as_ref() != Some(&worker_archive) {
                    return;
                }

                match result {
                    Ok(result) => {
                        worker_origin.entries.set(result.archive_entries);
                        worker_origin
                            .browser_entries
                            .update(|snapshot| snapshot.replace(result.browser_entries));
                        worker_status_bar.update(|status| {
                            status.message = if deleted_count == 1 {
                                "Deleted 1 file".to_string()
                            } else {
                                format!("Deleted {deleted_count} files")
                            };
                        });
                    }
                    Err(error) => {
                        let message = format!("Delete failed: {error}");
                        tracing::error!("{message}");
                        worker_status_bar.update(|status| status.message = message);
                    }
                }
            })
            .await;

            if let Err(error) = worker_result {
                if origin.archive_path.get().as_ref() == Some(&archive) {
                    let message = format!("archive delete worker failed: {error}");
                    tracing::error!("{message}");
                    status_bar.update(|status| status.message = message);
                }
            }
        });
    }

    pub fn extract(
        &self,
        shared: &SharedState,
        ops_state: &mut ArchiveOperationsState,
        file: &str,
    ) {
        // Per-row extract receives the stable archive-root path and
        // bypass selection entirely. extract_selected now takes a
        // list of paths so we just hand it `[file]` — no need to
        // synthesize a FileEntry or mutate the user's selection.
        let selected_paths = vec![file.to_string()];

        // extraction_dialog is per-tab now (post 2026-05-20 B3 reframed
        // slice 2). The extract call here originates from a row in the
        // active tab's archive list, so the dialog lives on the active
        // tab.
        let active_tab = shared.signals().tabs.get().active().clone();
        let mut temp_status = shared.signals().status_bar.get();
        let mut dialog = active_tab.extraction_dialog().get();

        operations::extraction::extract_selected(
            &shared.app_state,
            &selected_paths,
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

    pub fn read_text(&self, shared: &SharedState, origin: Arc<TabState>, path: String) {
        let io = Arc::new(BackendArchiveFileIo::capture(shared, &origin));
        self.read_text_with_io(shared, origin, path, io);
    }

    #[doc(hidden)]
    pub fn read_text_with_io(
        &self,
        shared: &SharedState,
        origin: Arc<TabState>,
        path: String,
        io: Arc<dyn ArchiveFileIo>,
    ) {
        let request_id = origin.file_request_seq.fetch_add(1, Ordering::Relaxed) + 1;
        origin.file_edit_dialog.update(|dialog| {
            dialog.show = true;
            dialog.full_path_in_archive = path.clone();
            dialog.name_input = path.clone();
            dialog.content.clear();
            dialog.original_content.clear();
            dialog.error.clear();
            dialog.load_state = FileEditLoadState::Loading { request_id };
        });

        let Some(archive) = origin.archive_path.get() else {
            let message = "No archive loaded".to_string();
            origin.file_edit_dialog.update(|dialog| {
                if dialog.full_path_in_archive == path
                    && dialog.load_state == (FileEditLoadState::Loading { request_id })
                {
                    dialog.error = message.clone();
                    dialog.load_state = FileEditLoadState::Failed(message.clone());
                }
            });
            shared.signals().status_bar.update(|status| {
                status.message = message;
            });
            return;
        };

        let runtime = shared.services.tokio_runtime.clone();
        let status_bar = shared.signals().status_bar.clone();
        let guard = OpGuard::new(&origin);

        runtime.spawn(async move {
            let _guard = guard;
            let worker_archive = archive.clone();
            let worker_path = path.clone();
            let result =
                tokio::task::spawn_blocking(move || io.read_text(&worker_archive, &worker_path))
                    .await
                    .map_err(|error| anyhow!("archive text worker failed: {error}"))
                    .and_then(|result| result);

            if origin.archive_path.get().as_ref() != Some(&archive) {
                return;
            }

            let mut applied_error = None;
            origin.file_edit_dialog.update(|dialog| {
                if dialog.full_path_in_archive != path
                    || dialog.load_state != (FileEditLoadState::Loading { request_id })
                {
                    return;
                }

                match result {
                    Ok(content) => {
                        dialog.content = content.clone();
                        dialog.original_content = content;
                        dialog.error.clear();
                        dialog.load_state = FileEditLoadState::Ready;
                    }
                    Err(error) => {
                        let message = format!("Failed to read file: {error}");
                        dialog.error = message.clone();
                        dialog.load_state = FileEditLoadState::Failed(message.clone());
                        applied_error = Some(message);
                    }
                }
            });

            if let Some(message) = applied_error {
                crate::core::utils::log_failure("FileEdit", &message);
                status_bar.update(|status| status.message = message);
            }
        });
    }

    pub fn copy_path(&self, egui_ctx: &egui::Context, file: &str) {
        egui_ctx.copy_text(file.to_string());
    }
}
