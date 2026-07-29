//! File operations application service

use crate::core::operations;
use crate::core::tabs::{OpGuard, TabState, ALL_ENTRIES_IN_ONE_DIRECTORY};
use crate::features::archive_operations::ArchiveOperationsState;
use crate::features::file_editing::domain::types::FileEditLoadState;
use crate::shared::SharedState;
use anyhow::{anyhow, Result};
use arclain_app::archive::{EntrySortKey, ListEntriesRequest, SortDirection};
use arclain_app::operations::ArchiveMutationRequest;
use arclain_core::backends::BackendSelector;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Narrow I/O boundary used to verify that a text read is scheduled
/// rather than executed by the caller. Public only because integration
/// tests compile as a separate crate.
///
/// Delete used to have an equivalent injectable seam here
/// (`delete_and_list`/`DeleteListResult`) before it moved onto
/// `arclain_app::ArclainApp::start_archive_mutation` (see
/// [`FileOpsService::delete_files`]'s own doc comment) -- that seam is
/// gone now: the facade's own integration tests
/// (`crates/app/tests/archive_mutation.rs`) exercise a real,
/// injectable-at-bootstrap fake backend, and this crate's own
/// `crates/ui/tests/` cover the UI-side wiring end to end against a real
/// bootstrapped `ArclainApp`. This trait stays narrowed to the one
/// synchronous read path that has not moved.
#[doc(hidden)]
pub trait TextReadIo: Send + Sync {
    fn read_text(&self, archive: &Path, path: &str) -> Result<String>;
}

struct BackendTextReadIo {
    backend_selector: BackendSelector,
    password: Option<String>,
}

impl BackendTextReadIo {
    fn capture(shared: &SharedState, origin: &TabState) -> Self {
        Self {
            backend_selector: shared.app_state.lock().backend_selector.clone(),
            password: origin.current_password.get(),
        }
    }
}

impl TextReadIo for BackendTextReadIo {
    fn read_text(&self, archive: &Path, path: &str) -> Result<String> {
        let backend = self.backend_selector.select(archive)?;
        backend.read_text_file(archive, path, self.password.as_deref())
    }
}

pub struct FileOpsService;

impl FileOpsService {
    /// Fire-and-forget: resolves `paths` (archive-relative path strings,
    /// scoped to `origin`'s currently-viewed directory -- the only rows a
    /// selection can ever contain) to their `EntryId`s, then submits a
    /// `DeleteEntries` mutation through the application facade at the
    /// resolved listing's own revision. The bridge
    /// (`crate::core::operation_bridge`) refreshes `origin`'s
    /// entries/browser_entries once the operation reaches `Completed`
    /// -- this method itself never touches those signals.
    ///
    /// Delete used to call `ArchiveBackend::delete_files` directly here,
    /// synchronously inside `spawn_blocking`, serialized per tab via
    /// `origin.archive_edit_lock` (a `parking_lot::Mutex` that also
    /// serialized this against a concurrent save-edit on the *same*
    /// tab, but not against a delete/edit on a *different* tab that
    /// happened to share the same archive file). `ArchiveSession::
    /// mutation_lock` (an async, session-scoped lock the facade now
    /// owns) supersedes that role: it serializes every mutation kind
    /// against every other one on the same session, regardless of which
    /// tab initiated it.
    pub fn delete_files(&self, shared: &SharedState, origin: Arc<TabState>, paths: Vec<String>) {
        if paths.is_empty() {
            shared.signals().status_bar.update(|status| {
                status.message = "No files selected".to_string();
            });
            return;
        }
        let Some(session_id) = origin.archive_session_id.get() else {
            shared.signals().status_bar.update(|status| {
                status.message = "No archive loaded".to_string();
            });
            return;
        };
        let Some(app) = shared.facade.clone() else {
            tracing::error!("[file_ops] delete_files: no application facade available");
            return;
        };

        let tab_id = origin.id;
        let directory = origin.listing.get().directory().clone();
        let shared = shared.clone();
        let runtime = shared.services.tokio_runtime.clone();
        runtime.spawn(async move {
            let page = match app
                .list_entries(
                    session_id,
                    ListEntriesRequest {
                        directory,
                        sort_key: EntrySortKey::Name,
                        sort_direction: SortDirection::Ascending,
                        name_filter: None,
                        offset: 0,
                        limit: ALL_ENTRIES_IN_ONE_DIRECTORY,
                    },
                )
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    tracing::error!("[file_ops] delete_files: list_entries failed: {error:?}");
                    shared.signals().status_bar.update(|status| {
                        status.message = format!("Delete failed: {}", error.summary);
                    });
                    return;
                }
            };

            let wanted: std::collections::HashSet<&str> =
                paths.iter().map(String::as_str).collect();
            let entry_ids: Vec<arclain_app::ids::EntryId> = page
                .entries
                .iter()
                .filter(|entry| wanted.contains(entry.path.as_str()))
                .map(|entry| entry.id)
                .collect();
            if entry_ids.is_empty() {
                // Every selected path has already disappeared from the
                // current listing (deleted by another action in the
                // meantime, or the selection was stale) -- nothing left
                // to delete.
                shared.signals().status_bar.update(|status| {
                    status.message = "No matching entries found to delete".to_string();
                });
                return;
            }

            let request = ArchiveMutationRequest::DeleteEntries {
                session_id,
                expected_revision: page.revision,
                entry_ids,
            };
            match app.start_archive_mutation(request).await {
                Ok(operation_id) => {
                    crate::core::operation_bridge::register_operation(
                        &shared,
                        operation_id,
                        tab_id,
                    )
                    .await;
                }
                Err(error) => {
                    tracing::error!(
                        "[file_ops] start_archive_mutation (DeleteEntries) was rejected: {error:?}"
                    );
                    shared.signals().status_bar.update(|status| {
                        status.message = format!("Delete failed: {}", error.summary);
                    });
                }
            }
        });
    }

    pub fn extract(
        &self,
        shared: &SharedState,
        _ops_state: &mut ArchiveOperationsState,
        file: &str,
    ) {
        // Per-row extract receives the stable archive-root path and
        // bypasses selection entirely — extract_selected takes a list
        // of paths so we just hand it `[file]`.
        let active_tab = shared.signals().tabs.get().active().clone();
        operations::extraction::extract_selected(shared, &active_tab, vec![file.to_string()]);
    }

    pub fn read_text(&self, shared: &SharedState, origin: Arc<TabState>, path: String) {
        let io = Arc::new(BackendTextReadIo::capture(shared, &origin));
        self.read_text_with_io(shared, origin, path, io);
    }

    #[doc(hidden)]
    pub fn read_text_with_io(
        &self,
        shared: &SharedState,
        origin: Arc<TabState>,
        path: String,
        io: Arc<dyn TextReadIo>,
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
