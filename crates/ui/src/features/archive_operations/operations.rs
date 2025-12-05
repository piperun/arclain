use super::ArchiveOperationsState;
use crate::core::utils;
use crate::platform::{resume_process, suspend_process};
use crate::shared::dialogs;

pub fn pause_extraction(state: &mut ArchiveOperationsState) {
    if let Some(child) = &state.extraction_child {
        let pid = child.id();
        if let Err(e) = suspend_process(pid) {
            tracing::error!("Failed to suspend process {}: {}", pid, e);
        } else {
            state.extraction_dialog.status = dialogs::ExtractionStatus::Paused;
        }
    }
}

pub fn resume_extraction(state: &mut ArchiveOperationsState) {
    if let Some(child) = &state.extraction_child {
        let pid = child.id();
        if let Err(e) = resume_process(pid) {
            tracing::error!("Failed to resume process {}: {}", pid, e);
        } else {
            state.extraction_dialog.status = dialogs::ExtractionStatus::Running;
        }
    }
}

pub fn cancel_extraction(state: &mut ArchiveOperationsState) {
    if let Some(mut child) = state.extraction_child.take() {
        if let Err(e) = child.kill() {
            tracing::error!("Failed to kill process: {}", e);
        }
        state.extraction_dialog.status = dialogs::ExtractionStatus::Cancelled;
        state.extraction_rx = None;
        state.extraction_started = None;
    }
}

pub fn update_extraction_progress(state: &mut ArchiveOperationsState, ctx: &egui::Context) {
    if let Some(rx) = &state.extraction_rx {
        for upd in rx.try_iter() {
            if upd.percent > 0 {
                state.extraction_dialog.percent = upd.percent;
            }
            if let Some(msg) = upd.message {
                // Keep last ~500 lines
                if state.extraction_dialog.log_lines.len() > 500 {
                    let overflow = state.extraction_dialog.log_lines.len() - 500;
                    state.extraction_dialog.log_lines.drain(0..overflow);
                }
                state.extraction_dialog.log_lines.push(msg);
            }
            if let Some(start) = state.extraction_started {
                let elapsed = start.elapsed();
                state.extraction_dialog.elapsed_text = utils::format_duration(elapsed);
                if upd.percent > 0 && upd.percent < 100 {
                    let total_est = elapsed.mul_f64(100.0 / upd.percent as f64);
                    let left = total_est.saturating_sub(elapsed);
                    state.extraction_dialog.time_left_text = utils::format_duration(left);
                    state.extraction_dialog.processed_text = format!("{}%", upd.percent);
                }
            }
            ctx.request_repaint();
        }
    }

    // Check child completion
    if let Some(child) = state.extraction_child.as_mut() {
        if let Ok(Some(status)) = child.try_wait() {
            if status.success() && state.extraction_dialog.percent >= 100 {
                state.extraction_dialog.status = dialogs::ExtractionStatus::Completed;
            } else {
                state.extraction_dialog.status = dialogs::ExtractionStatus::Failed;
            }
            // Auto-hide when completed unless minimized
            if !state.extraction_minimized {
                state.extraction_dialog.show = false;
            }
            state.extraction_child = None;
            state.extraction_rx = None;
            state.extraction_started = None;
        }
    }
}

pub fn update_conversion_progress(state: &mut ArchiveOperationsState, ctx: &egui::Context) {
    if let Some(rx) = &state.conversion_rx {
        for upd in rx.try_iter() {
            if upd.percent > 0 {
                state.conversion_dialog.percent = upd.percent;
            }
            if let Some(msg) = upd.message {
                // Keep last ~500 lines
                if state.conversion_dialog.log_lines.len() > 500 {
                    let overflow = state.conversion_dialog.log_lines.len() - 500;
                    state.conversion_dialog.log_lines.drain(0..overflow);
                }
                state.conversion_dialog.log_lines.push(msg);
            }
            if let Some(start) = state.conversion_started {
                let elapsed = start.elapsed();
                state.conversion_dialog.elapsed_text = utils::format_duration(elapsed);
                if upd.percent > 0 && upd.percent < 100 {
                    let total_est = elapsed.mul_f64(100.0 / upd.percent as f64);
                    let left = total_est.saturating_sub(elapsed);
                    state.conversion_dialog.time_left_text = utils::format_duration(left);
                    state.conversion_dialog.processed_text = format!("{}%", upd.percent);
                }
            }
            ctx.request_repaint();
        }
    }

    // Check conversion child completion
    if let Some(child) = state.conversion_child.as_mut() {
        if let Ok(Some(status)) = child.try_wait() {
            if status.success() && state.conversion_dialog.percent >= 100 {
                state.conversion_dialog.status = dialogs::ExtractionStatus::Completed;
            } else {
                state.conversion_dialog.status = dialogs::ExtractionStatus::Failed;
            }
            // Auto-hide when completed unless minimized
            if !state.conversion_minimized {
                state.conversion_dialog.show = false;
            }
            state.conversion_child = None;
            state.conversion_rx = None;
            state.conversion_started = None;
        }
    }
}
