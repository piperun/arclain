//! Extraction as an application-facade operation.
//!
//! The facade owns process spawning and cancellation for the CLI
//! extraction this drives (`arclain_app::operations::extract`) --
//! egui no longer holds a `std::process::Child` directly. Progress,
//! challenges (a wrong-password retry), and completion all route back
//! onto `tab.extraction_dialog()` through
//! `crate::core::operation_bridge`.

use crate::core::tabs::TabState;
use crate::shared::SharedState;
use std::sync::Arc;

/// Starts extracting `entry_paths` (empty means the whole archive) from
/// `tab`'s open archive into a user-picked destination folder.
/// Fire-and-forget: resolves the archive-relative paths to the
/// facade's `EntryId`s (via `list_entries` on the tab's current
/// directory -- selection is always scoped to entries visible in that
/// directory, matching the pre-facade UI's own selection model), then
/// dispatches `start_extract` and registers the resulting operation with
/// the bridge.
pub fn start_extraction(shared: &SharedState, tab: &Arc<TabState>, entry_paths: Vec<String>) {
    if tab.extraction_dialog().get().show {
        shared.signals().status_bar.update(|s| {
            s.message = "Another extraction is already running".to_string();
        });
        return;
    }
    let Some(session_id) = tab.archive_session_id.get() else {
        shared.signals().status_bar.update(|s| {
            s.message = "No archive open".to_string();
        });
        return;
    };
    let Some(destination) = rfd::FileDialog::new().pick_folder() else {
        return;
    };
    let Some(app) = shared.facade.clone() else {
        tracing::error!("[extraction] start_extraction: no application facade available");
        return;
    };

    let origins = shared.operation_origins.clone();
    let runtime = shared.services.tokio_runtime.clone();
    let shared = shared.clone();
    let tab_id = tab.id;
    let tab = tab.clone();
    let current_directory = tab.navigation.get().current_path.clone();

    {
        let mut dialog = tab.extraction_dialog().get();
        dialog.show = true;
        dialog.title = format!("Extracting to {}", destination.display());
        dialog.percent = 0;
        dialog.status = crate::shared::dialogs::ExtractionStatus::Running;
        // No facade-level pause/minimize primitive exists (only
        // cancellation) -- disable both rather than leave them silently
        // non-functional. See `crate::core::arclain_app::dialog_handler`'s
        // own comment on the now-inert `Minimized`/`Paused`/`Resumed`
        // dialog results.
        dialog.can_pause = false;
        dialog.can_minimize = false;
        dialog.can_cancel = true;
        dialog.log_lines.clear();
        tab.extraction_dialog().set(dialog);
    }

    runtime.spawn(async move {
        let entry_ids = if entry_paths.is_empty() {
            Vec::new()
        } else {
            let directory = arclain_app::archive::ArchivePath::parse(current_directory)
                .unwrap_or_else(|_| arclain_app::archive::ArchivePath::root());
            let page = match app
                .list_entries(
                    session_id,
                    arclain_app::archive::ListEntriesRequest {
                        directory,
                        sort_key: arclain_app::archive::EntrySortKey::Name,
                        sort_direction: arclain_app::archive::SortDirection::Ascending,
                        name_filter: None,
                        offset: 0,
                        limit: 100_000,
                    },
                )
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    tracing::error!("[extraction] failed to resolve selected entries: {error:?}");
                    shared.signals().status_bar.update(|s| {
                        s.message = format!("Extraction failed: {error:?}");
                    });
                    return;
                }
            };
            entry_paths
                .iter()
                .filter_map(|selected| {
                    page.entries
                        .iter()
                        .find(|entry| entry.path.as_str() == selected)
                        .map(|entry| entry.id)
                })
                .collect()
        };

        match app
            .start_extract(arclain_app::operations::ExtractRequest {
                session_id,
                entry_ids,
                destination,
                collision_policy: arclain_app::operations::CollisionPolicy::Overwrite,
            })
            .await
        {
            Ok(operation_id) => {
                origins.register(operation_id, tab_id);
                tab.active_extraction_operation.set(Some(operation_id));
            }
            Err(error) => {
                tracing::error!("[extraction] start_extract was rejected: {error:?}");
                shared.signals().status_bar.update(|s| {
                    s.message = format!("Failed to start extraction: {error:?}");
                });
                let mut dialog = tab.extraction_dialog().get();
                dialog.show = false;
                tab.extraction_dialog().set(dialog);
            }
        }
    });
}

/// Extract a set of files identified by archive-root-relative paths from
/// the active tab's archive.
///
/// Callers pre-compute the list of paths — either by filtering the
/// active tab's `browser_entries` against `browser_view_state.selection`
/// (toolbar "Extract" button), or by passing a single path from a
/// per-row action (`file_ops_service::extract`).
pub fn extract_selected(shared: &SharedState, tab: &Arc<TabState>, selected_files: Vec<String>) {
    if selected_files.is_empty() {
        shared.signals().status_bar.update(|s| {
            s.message = "No files selected".to_string();
        });
        return;
    }
    start_extraction(shared, tab, selected_files);
}

/// Extract all files from the archive.
pub fn extract_all(shared: &SharedState, tab: &Arc<TabState>) {
    start_extraction(shared, tab, Vec::new());
}

/// Cancels the extraction currently running for `tab`, if any.
pub fn cancel_extraction(shared: &SharedState, tab: &Arc<TabState>) {
    let Some(operation_id) = tab.active_extraction_operation.get() else {
        return;
    };
    let Some(app) = shared.facade.clone() else {
        return;
    };
    let runtime = shared.services.tokio_runtime.clone();
    runtime.spawn(async move {
        let _ = app.cancel_operation(operation_id).await;
    });
}
