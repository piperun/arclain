use crate::core::utils;
use crate::features::archive_operations::domain::state::ArchiveOperationsState;
use crate::platform::{resume_process, suspend_process};
use crate::shared::dialogs;
use eframe::egui;
use std::sync::atomic::Ordering;

pub fn pause_extraction(state: &mut ArchiveOperationsState, _shared: &crate::shared::SharedState) {
    if let Some(child) = &state.extraction_child {
        let pid = child.id();
        if let Err(e) = suspend_process(pid) {
            tracing::error!("Failed to suspend process {}: {}", pid, e);
        } else if let Some(origin_tab) = &state.extraction_origin_tab {
            // Dialog is per-tab now (post 2026-05-20 B3 reframed slice 2);
            // address the origin tab directly via the captured Arc.
            let mut dialog = origin_tab.extraction_dialog().get();
            dialog.status = dialogs::ExtractionStatus::Paused;
            origin_tab.extraction_dialog().set(dialog);
        }
    }
}

pub fn resume_extraction(state: &mut ArchiveOperationsState, _shared: &crate::shared::SharedState) {
    if let Some(child) = &state.extraction_child {
        let pid = child.id();
        if let Err(e) = resume_process(pid) {
            tracing::error!("Failed to resume process {}: {}", pid, e);
        } else if let Some(origin_tab) = &state.extraction_origin_tab {
            let mut dialog = origin_tab.extraction_dialog().get();
            dialog.status = dialogs::ExtractionStatus::Running;
            origin_tab.extraction_dialog().set(dialog);
        }
    }
}

pub fn cancel_extraction(state: &mut ArchiveOperationsState, _shared: &crate::shared::SharedState) {
    if let Some(mut child) = state.extraction_child.take() {
        if let Err(e) = child.kill() {
            tracing::error!("Failed to kill process: {}", e);
        }
        // Write the Cancelled status to the origin tab's dialog before we
        // drop the origin handle below — once the Arc is gone we can't
        // reach the right tab anymore.
        if let Some(origin_tab) = &state.extraction_origin_tab {
            let mut dialog = origin_tab.extraction_dialog().get();
            dialog.status = dialogs::ExtractionStatus::Cancelled;
            origin_tab.extraction_dialog().set(dialog);
        }

        state.extraction_rx = None;
        state.extraction_started = None;
        // Drop guard and origin tab: decrements in_flight_ops.
        state.extraction_op_guard = None;
        state.extraction_origin_tab = None;
    }
}

pub fn update_extraction_progress(
    state: &mut ArchiveOperationsState,
    _shared: &crate::shared::SharedState,
    ctx: &egui::Context,
) {
    // Without an origin tab there's no dialog to update — bail out
    // early. (Pre-2026-05-20 the dialog lived on a global signal so
    // it was always accessible; now it lives on the originating tab.)
    let Some(origin_tab) = state.extraction_origin_tab.clone() else {
        return;
    };

    // Cooperative cancellation: if the originating tab was force-closed, kill
    // the subprocess and clean up so the op counter drops to zero promptly.
    if origin_tab.tab_cancel.load(Ordering::SeqCst) {
        if let Some(mut child) = state.extraction_child.take() {
            let _ = child.kill();
        }
        state.extraction_rx = None;
        state.extraction_started = None;
        state.extraction_op_guard = None;
        state.extraction_origin_tab = None;
        // The tab itself was force_close'd, so its dialog state will die
        // with it when the TabsCollection drops the Arc. No need to set
        // show=false on a tab that's already being removed.
        return;
    }

    let mut dialog = origin_tab.extraction_dialog().get();
    let mut changed = false;

    if let Some(rx) = &state.extraction_rx {
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
            if let Some(start) = state.extraction_started {
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

    // Check child completion
    if let Some(child) = state.extraction_child.as_mut() {
        if let Ok(Some(status)) = child.try_wait() {
            changed = true;
            if status.success() && dialog.percent >= 100 {
                dialog.status = dialogs::ExtractionStatus::Completed;
            } else {
                dialog.status = dialogs::ExtractionStatus::Failed;
            }
            // Auto-hide when completed unless minimized
            if !state.extraction_minimized {
                dialog.show = false;
            }
            state.extraction_child = None;
            state.extraction_rx = None;
            state.extraction_started = None;
            // Drop guard and origin tab: decrements in_flight_ops.
            state.extraction_op_guard = None;
            state.extraction_origin_tab = None;
        }
    }

    if changed {
        origin_tab.extraction_dialog().set(dialog);
    }
}
