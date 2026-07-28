//! Drag and drop application service.
//!
//! Deliberately **not** routed through `arclain_app::materialization` in
//! this task, unlike `file_opener::open_file_from_archive`. Investigated
//! and rejected, rather than silently left alone:
//!
//! - **No leak exists here to fix.** This function reaches
//!   `crate::platform::drag_source::start_deferred_drag`, whose Windows
//!   implementation (`platform::drag_source::windows::{data_object,
//!   hdrop_data_object}`) already owns its own temp directory via
//!   `tempfile::TempDir` -- ordinary RAII, cleaned up when the OS releases
//!   the `IDataObject` COM object after `DoDragDrop` returns. There is no
//!   `std::mem::forget` (or equivalent unbounded retention) anywhere in
//!   that path; verified by reading the implementation, not assumed.
//! - **Migrating it anyway would require a real redesign, not a small
//!   change.** The Windows drag objects extract *lazily*, on demand,
//!   inside a `GetData` callback invoked by the OS shell during the drag
//!   gesture itself (`HDropDataObject`'s dual-HDROP: a placeholder path
//!   during hover, the real extracted files only once an actual drop
//!   happens) -- a deliberate, working responsiveness optimization.
//!   Routing that through `start_materialization` would mean either
//!   eagerly materializing the whole selection before the OS drag even
//!   starts (giving up the "don't extract until actually dropped"
//!   behavior), or bridging a synchronous, OS-invoked COM callback into
//!   the facade's async operation model with no render loop on the other
//!   end to hand off to -- a harder version of the same "can't block the
//!   render thread on an async challenge" problem `start_open_archive`'s
//!   own background-worker bridge exists to solve, except here there is no
//!   egui frame loop backing the callback thread at all.
//! - **The one real, pre-existing architectural gap** -- this function
//!   reads `tab.opened_archive` directly for `backend_arc()`/`password_ref()`,
//!   bypassing the facade entirely (flagged by an earlier task's own
//!   report as a known, deliberately deferred item) -- is not created by
//!   this task and is not fixed by it either: the facade has no API that
//!   hands back a raw backend handle for a UI-driven lazy extraction to
//!   use (by design), so closing this gap needs the redesign above, not a
//!   small follow-up.
//!
//! Given no leak to fix and a disproportionate, high-risk redesign to
//! reach full architectural parity, this file is unchanged. Full
//! unification remains a named, surfaced follow-up.

use crate::features::archive_operations::ArchiveOperationsState;
use crate::shared::SharedState;

pub struct DragDropService;

impl DragDropService {
    pub fn drag_extract(
        &self,
        shared: &SharedState,
        ops_state: &mut ArchiveOperationsState,
        files: Vec<String>,
    ) {
        tracing::info!("[DragExtract] Starting with files: {}", files.join(", "));

        // Get archive handle. The drag dialog (per-tab post 2026-05-20 B3
        // reframed slice 2) also lives on this same tab — the drag op is
        // initiated by selecting rows in the active tab's archive list.
        let tab = shared.signals().tabs.get().active().clone();
        let archive_guard = tab.opened_archive.read();
        let archive_arc_opt = archive_guard.as_ref().cloned();
        drop(archive_guard);

        if let Some(archive_arc) = archive_arc_opt {
            let archive = archive_arc.read();

            // Collect entries matching the dragged files (or directory contents)
            let all_entries = tab.entries.get();
            tracing::info!(
                "[DragExtract] Total entries in archive: {}",
                all_entries.len()
            );

            // Match entries: exact match OR starts with "folder/" OR starts with "folder\"
            let entries: Vec<arclain_core::ArchiveEntry> = all_entries
                .iter()
                .filter(|e| {
                    files.iter().any(|f| {
                        if e.path == *f {
                            return true;
                        }
                        if e.path.starts_with(&format!("{}/", f)) {
                            return true;
                        }
                        if e.path.starts_with(&format!("{}\\", f)) {
                            return true;
                        }
                        false
                    })
                })
                .cloned()
                .collect();

            tracing::info!("[DragExtract] Matched {} entries for drag", entries.len());

            let backend = archive.backend_arc();
            let archive_path = archive.path().to_path_buf();
            let password = archive.password_ref().map(|p| p.to_string());

            drop(archive); // Release lock before drag loop blocks

            if entries.is_empty() {
                tracing::warn!("[DragExtract] No matching entries found");
                return;
            }

            // Start the drag operation
            match crate::platform::drag_source::start_deferred_drag(
                backend,
                archive_path,
                entries,
                password,
            ) {
                Ok(rx) => {
                    tracing::info!("[DragExtract] Drag operation started in background");
                    shared.signals().status_bar.update(|s| {
                        s.message = "Drag started...".to_string();
                    });

                    ops_state.drag_rx = Some(rx);
                    // Capture the origin tab so the background drag updater
                    // can route progress writes back to this tab's dialog
                    // slot (post 2026-05-20 B3 reframed slice 2).
                    ops_state.drag_origin_tab = Some(tab.clone());

                    let mut dialog = tab.drag_dialog().get();
                    dialog.show = false;
                    dialog.percent = 0;
                    dialog.file_action = "Preparing drag...".to_string();
                    tab.drag_dialog().set(dialog);

                    ops_state.drag_started = Some(std::time::Instant::now());
                }
                Err(e) => {
                    tracing::warn!("[DragExtract] Drag failed: {}", e);
                    shared.signals().status_bar.update(|s| {
                        s.message = format!("Drag failed: {}", e);
                    });
                }
            }
        } else {
            tracing::warn!("[DragExtract] No archive session open");
            shared.signals().status_bar.update(|s| {
                s.message = "No archive open".to_string();
            });
        }
    }
}
