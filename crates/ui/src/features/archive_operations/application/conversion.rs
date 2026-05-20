use crate::core::utils;
use crate::features::archive_operations::domain::state::ArchiveOperationsState;
use crate::shared::dialogs;
use eframe::egui;
use std::sync::atomic::Ordering;

pub fn update_conversion_progress(
    state: &mut ArchiveOperationsState,
    _shared: &crate::shared::SharedState,
    ctx: &egui::Context,
) {
    // Without an origin tab there's no dialog to update — bail out
    // early. Post 2026-05-20 B3 reframed slice 2 the conversion
    // dialog lives on the originating tab; no origin = no target.
    let Some(origin_tab) = state.conversion_origin_tab.clone() else {
        return;
    };

    // Cooperative cancellation: if the originating tab was force-closed, kill
    // the subprocess and clean up so the op counter drops to zero promptly.
    if origin_tab.tab_cancel.load(Ordering::SeqCst) {
        if let Some(mut child) = state.conversion_child.take() {
            let _ = child.kill();
        }
        state.conversion_rx = None;
        state.conversion_started = None;
        state.conversion_op_guard = None;
        state.conversion_origin_tab = None;
        // The tab is being force-closed — its dialog will die with it.
        return;
    }

    let mut dialog = origin_tab.conversion_dialog().get();
    let mut changed = false;

    if let Some(rx) = &state.conversion_rx {
        for upd in rx.try_iter() {
            changed = true;
            if upd.percent > 0 {
                dialog.percent = upd.percent;
            }
            if let Some(msg) = upd.message {
                // Keep last ~500 lines
                if dialog.log_lines.len() > 500 {
                    let overflow = dialog.log_lines.len() - 500;
                    dialog.log_lines.drain(0..overflow);
                }
                dialog.log_lines.push(msg);
            }
            if let Some(start) = state.conversion_started {
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
    }

    // Check conversion child completion
    if let Some(child) = state.conversion_child.as_mut() {
        if let Ok(Some(status)) = child.try_wait() {
            changed = true;
            if status.success() && dialog.percent >= 100 {
                dialog.status = dialogs::ExtractionStatus::Completed;
            } else {
                dialog.status = dialogs::ExtractionStatus::Failed;
            }
            // Auto-hide when completed unless minimized
            if !state.conversion_minimized {
                dialog.show = false;
            }
            state.conversion_child = None;
            state.conversion_rx = None;
            state.conversion_started = None;
            // Drop guard and origin tab: decrements in_flight_ops.
            state.conversion_op_guard = None;
            state.conversion_origin_tab = None;
        }
    }

    if changed {
        origin_tab.conversion_dialog().set(dialog);
    }
}
