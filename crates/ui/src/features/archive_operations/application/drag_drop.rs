use crate::core::utils;
use crate::features::archive_operations::domain::state::ArchiveOperationsState;
use eframe::egui;

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
