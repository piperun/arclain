//! Progress polling for an OS-level drag-out already in flight (started by
//! `crate::features::archive_browser::application::drag_drop_service`).
//!
//! This module's own state (`ArchiveOperationsState::drag_rx`/
//! `drag_origin_tab`) holds no filesystem resource -- just an
//! `mpsc::Receiver` of `DragProgressUpdate`s and a tab handle. The staged
//! files themselves live in a facade materialization lease owned by the
//! drag's COM data object (see `drag_drop_service`'s own module doc
//! comment); the channel disconnecting is still this updater's "drag
//! finished, close the dialog" signal, exactly as before the facade
//! cutover.

use crate::core::utils;
use crate::features::archive_operations::domain::state::ArchiveOperationsState;
use eframe::egui;
use std::path::Path;

/// Whether a file dropped onto the app window should be routed into
/// `AddFiles` against the active tab's open archive, rather than the
/// existing "open a dropped archive" routing
/// (`crate::core::arclain_app::dialog_handler::render_overlays`'s own
/// zone/`DropBehavior` logic, unchanged by this task).
///
/// True exactly when an archive is already open in the active tab *and*
/// the dropped path is not itself a recognized archive extension.
/// `render_overlays` partitions every drop gesture's paths through this
/// check before doing anything else: paths this returns `true` for go
/// straight to `crate::core::operations::file::start_add_files`; every
/// other path keeps going through the existing open/replace-tab zone
/// routing exactly as before. Opening a dropped *archive* -- including
/// one dropped while another is already open, which replaces or adds a
/// tab -- remains that existing, unchanged frontend action; this
/// function only decides the other half of the fork.
pub fn should_add_to_open_archive(
    dropped_path: &Path,
    active_archive_session_id: Option<arclain_app::ids::ArchiveSessionId>,
) -> bool {
    active_archive_session_id.is_some() && !crate::core::file_drop::is_archive(dropped_path)
}

pub fn update_drag_progress(
    state: &mut ArchiveOperationsState,
    _shared: &crate::shared::SharedState,
    ctx: &egui::Context,
) {
    // Drag dialog lives on the originating tab now (post 2026-05-20 B3
    // reframed slice 2). Without an origin tab there's no dialog to
    // update — bail out early.
    let Some(origin_tab) = state.drag_origin_tab.clone() else {
        return;
    };

    let mut dialog = origin_tab.drag_dialog().get();
    let mut changed = false;
    let mut finished = false;

    if let Some(rx) = &state.drag_rx {
        for upd in rx.try_iter() {
            changed = true;
            // Auto-show dialog on first progress update
            if !dialog.show {
                dialog.show = true;
                state.drag_started = Some(std::time::Instant::now());
            }

            if upd.percent > 0 {
                dialog.percent = upd.percent;
            }
            if let Some(msg) = upd.message {
                // Keep last ~50 lines
                if dialog.log_lines.len() > 50 {
                    let overflow = dialog.log_lines.len() - 50;
                    dialog.log_lines.drain(0..overflow);
                }
                dialog.log_lines.push(msg);
            }
            if let Some(start) = state.drag_started {
                let elapsed = start.elapsed();
                dialog.elapsed_text = utils::format_duration(elapsed);
                if upd.percent > 0 && upd.percent < 100 {
                    let total_est = elapsed.mul_f64(100.0 / upd.percent as f64);
                    let left = total_est.saturating_sub(elapsed);
                    dialog.time_left_text = utils::format_duration(left);
                    dialog.processed_text = format!("{}%", upd.percent);
                }
            }
            ctx.request_repaint();
        }

        // Check if finished (channel disconnected means thread ended)
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                finished = true;
            }
            _ => {}
        }
    }

    if finished {
        dialog.show = false;
        state.drag_rx = None;
        state.drag_started = None;
        // Origin tab handle is dropped on completion — mirrors
        // extraction/conversion cleanup. Holds the Arc until the
        // updater observes "rx disconnected".
        state.drag_origin_tab = None;
        changed = true;
    }

    if changed {
        origin_tab.drag_dialog().set(dialog);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_non_archive_dropped_onto_an_open_archive_is_routed_to_add() {
        assert!(should_add_to_open_archive(
            &PathBuf::from("notes.txt"),
            Some(arclain_app::ids::ArchiveSessionId::from_raw(1))
        ));
    }

    #[test]
    fn a_recognized_archive_is_never_routed_to_add_even_with_one_already_open() {
        assert!(!should_add_to_open_archive(
            &PathBuf::from("other.zip"),
            Some(arclain_app::ids::ArchiveSessionId::from_raw(1))
        ));
    }

    #[test]
    fn a_non_archive_with_no_archive_open_is_never_routed_to_add() {
        assert!(!should_add_to_open_archive(
            &PathBuf::from("notes.txt"),
            None
        ));
    }
}
