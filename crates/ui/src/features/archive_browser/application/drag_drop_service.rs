//! Drag and drop application service

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
