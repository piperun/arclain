use crate::core::utils;
use crate::features::archive_operations::domain::state::ArchiveOperationsState;
use crate::platform::{resume_process, suspend_process};
use crate::shared::dialogs;
use eframe::egui;

pub fn pause_extraction(state: &mut ArchiveOperationsState, shared: &crate::shared::SharedState) {
    if let Some(child) = &state.extraction_child {
        let pid = child.id();
        if let Err(e) = suspend_process(pid) {
            tracing::error!("Failed to suspend process {}: {}", pid, e);
        } else {
            let mut dialog = shared.signals().extraction_dialog.get();
            dialog.status = dialogs::ExtractionStatus::Paused;
            shared.signals().extraction_dialog.set(dialog);
        }
    }
}

pub fn resume_extraction(state: &mut ArchiveOperationsState, shared: &crate::shared::SharedState) {
    if let Some(child) = &state.extraction_child {
        let pid = child.id();
        if let Err(e) = resume_process(pid) {
            tracing::error!("Failed to resume process {}: {}", pid, e);
        } else {
            let mut dialog = shared.signals().extraction_dialog.get();
            dialog.status = dialogs::ExtractionStatus::Running;
            shared.signals().extraction_dialog.set(dialog);
        }
    }
}

pub fn cancel_extraction(state: &mut ArchiveOperationsState, shared: &crate::shared::SharedState) {
    if let Some(mut child) = state.extraction_child.take() {
        if let Err(e) = child.kill() {
            tracing::error!("Failed to kill process: {}", e);
        }
        let mut dialog = shared.signals().extraction_dialog.get();
        dialog.status = dialogs::ExtractionStatus::Cancelled;
        shared.signals().extraction_dialog.set(dialog);

        state.extraction_rx = None;
        state.extraction_started = None;
    }
}

pub fn update_extraction_progress(
    state: &mut ArchiveOperationsState,
    shared: &crate::shared::SharedState,
    ctx: &egui::Context,
) {
    let mut dialog = shared.signals().extraction_dialog.get();
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
        }
    }

    if changed {
        shared.signals().extraction_dialog.set(dialog);
    }
}
