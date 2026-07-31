//! Drag and drop application service.
//!
//! Routed through the application facade's drag-stage surface
//! (`arclain_app::ArclainApp::{start_drag_stage, stage_drag_payload_blocking}`),
//! closing the last place this crate reached `arclain_core` through
//! `tab.opened_archive` for a backend handle. The division of labor:
//!
//! - **This service** (drag start, synchronous): snapshots the active
//!   tab's `archive_session_id`, hands the drag progress channel to the
//!   per-frame updater, and spawns one async task that resolves the
//!   dragged paths to facade `EntryId`s (a single `list_entries` of the
//!   browsed directory -- every draggable row lives in it) before
//!   starting the OS drag thread.
//! - **The platform layer** (`crate::platform::drag_source`): owns all
//!   COM mechanics. During hover it serves a placeholder HDROP and makes
//!   **no** facade calls -- the hover-then-extract optimization is
//!   untouched by the cutover. Only when the shell commits to a drop
//!   does the drag's own STA thread call the facade's blocking staging
//!   affordance through [`FacadeDragPayloadSource`].
//!
//! Folder rows are passed as their own single `EntryId`; the facade
//! expands them to their subtrees server-side (the prefix-matching this
//! service used to do against `tab.entries` moved behind the facade with
//! the extraction it fed).

use std::sync::Arc;

use crate::features::archive_operations::ArchiveOperationsState;
use crate::platform::drag_source::{DragPayloadSource, FacadeDragPayloadSource};
use crate::shared::SharedState;
use arclain_app::archive::ArchivePath;
use arclain_app::ids::{ArchiveSessionId, EntryId};
use arclain_app::ArclainApp;

/// The parent directory (archive-root-relative, forward-slash form) the
/// dragged rows live in. Every row of one drag gesture comes from the
/// single directory the browser is showing, so the first path's parent
/// is everyone's parent.
fn parent_directory(files: &[String]) -> String {
    let first = files
        .first()
        .map(|path| path.replace('\\', "/"))
        .unwrap_or_default();
    match first.rfind('/') {
        Some(pos) => first[..pos].to_string(),
        None => String::new(),
    }
}

/// Resolves each dragged archive-root path to the `EntryId` naming it,
/// through one listing of the browsed directory. A path naming both a
/// file and a directory at the same level (a real, allowed collision --
/// see `arclain_app::archive`'s entry-identity notes) contributes both
/// ids, mirroring the pre-facade prefix matcher which also picked up
/// both.
async fn resolve_selection_entry_ids(
    app: &ArclainApp,
    session_id: ArchiveSessionId,
    files: &[String],
) -> Result<Vec<EntryId>, String> {
    let directory = ArchivePath::parse(parent_directory(files))
        .map_err(|error| format!("invalid drag directory: {error:?}"))?;
    let page = app
        .list_entries(
            session_id,
            arclain_app::archive::ListEntriesRequest::whole_directory(directory),
        )
        .await
        .map_err(|error| error.summary)?;

    let mut entry_ids = Vec::with_capacity(files.len());
    for file in files {
        let normalized = file.replace('\\', "/");
        let mut found = false;
        for entry in &page.entries {
            if entry.path.as_str() == normalized {
                entry_ids.push(entry.id);
                found = true;
            }
        }
        if !found {
            return Err(format!(
                "{normalized} is not in the current directory listing"
            ));
        }
    }
    Ok(entry_ids)
}

pub struct DragDropService;

impl DragDropService {
    pub fn drag_extract(
        &self,
        shared: &SharedState,
        ops_state: &mut ArchiveOperationsState,
        files: Vec<String>,
    ) {
        tracing::info!("[DragExtract] Starting with files: {}", files.join(", "));

        let tab = shared.signals().tabs.get().active().clone();
        let Some(session_id) = tab.archive_session_id.get() else {
            tracing::warn!("[DragExtract] No archive session open");
            shared.signals().status_bar.update(|s| {
                s.message = "No archive open".to_string();
            });
            return;
        };
        let Some(app) = shared.facade.clone() else {
            tracing::error!("[DragExtract] No application facade available");
            return;
        };
        if files.is_empty() {
            tracing::warn!("[DragExtract] Empty drag selection");
            return;
        }

        // Progress channel + origin tab are installed synchronously, so
        // the per-frame updater (`update_drag_progress`) owns the drag
        // dialog from this frame on -- the exact pre-facade shape. The
        // channel disconnecting (every sender dropped, including on the
        // failure paths below) is its cleanup signal.
        let (tx, rx) = std::sync::mpsc::channel();
        ops_state.drag_rx = Some(rx);
        ops_state.drag_origin_tab = Some(tab.clone());
        ops_state.drag_started = Some(std::time::Instant::now());

        let mut dialog = tab.drag_dialog().get();
        dialog.show = false;
        dialog.percent = 0;
        dialog.file_action = "Preparing drag...".to_string();
        tab.drag_dialog().set(dialog);

        shared.signals().status_bar.update(|s| {
            s.message = "Drag started...".to_string();
        });

        let runtime_handle = shared.services.tokio_runtime.handle().clone();
        let shared_for_task = shared.clone();
        shared
            .services
            .tokio_runtime
            .handle()
            .clone()
            .spawn(async move {
                let entry_ids = match resolve_selection_entry_ids(&app, session_id, &files).await {
                    Ok(entry_ids) => entry_ids,
                    Err(message) => {
                        tracing::warn!(
                            "[DragExtract] Failed to resolve dragged entries: {message}"
                        );
                        shared_for_task.signals().status_bar.update(|s| {
                            s.message = format!("Drag failed: {message}");
                        });
                        return; // `tx` drops; the updater sees disconnect.
                    }
                };

                tracing::info!(
                    "[DragExtract] Resolved {} entry ids for drag",
                    entry_ids.len()
                );

                let source: Arc<dyn DragPayloadSource> = Arc::new(FacadeDragPayloadSource::new(
                    app,
                    runtime_handle,
                    session_id,
                    entry_ids,
                ));

                match crate::platform::drag_source::start_deferred_drag(source, files, tx) {
                    Ok(()) => {
                        tracing::info!("[DragExtract] Drag operation started in background");
                    }
                    Err(e) => {
                        tracing::warn!("[DragExtract] Drag failed: {}", e);
                        shared_for_task.signals().status_bar.update(|s| {
                            s.message = format!("Drag failed: {}", e);
                        });
                    }
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::parent_directory;

    #[test]
    fn parent_directory_of_root_level_rows_is_the_archive_root() {
        assert_eq!(parent_directory(&["readme.txt".to_string()]), "");
    }

    #[test]
    fn parent_directory_of_nested_rows_is_their_containing_directory() {
        assert_eq!(
            parent_directory(&["game/data/save.dat".to_string()]),
            "game/data"
        );
        // Backslash-separated paths normalize the same way.
        assert_eq!(
            parent_directory(&["game\\data\\save.dat".to_string()]),
            "game/data"
        );
    }
}
