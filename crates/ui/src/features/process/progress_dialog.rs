//! Modal progress dialog for pipeline runs.

use crate::core::signals::ProcessRunState;
use crate::shared::dialogs::helpers::{show_dimmed_modal, ModalParams};
use arclain_theme::AppTheme;
use arclain_widgets::{ButtonSize, Text, TextButton};
use eframe::egui;

/// What the user asked of the dialog this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessProgressResult {
    /// Cancel the in-flight run. The caller routes this to
    /// `crate::core::operations::process_runner::cancel_pipeline_run`,
    /// which reaches the operation registry — the dialog itself never
    /// touches the application.
    Cancel,
    /// Dismiss the completed run's summary.
    Close,
}

pub fn render(
    ctx: &egui::Context,
    theme: &AppTheme,
    state: &ProcessRunState,
) -> Option<ProcessProgressResult> {
    // Only show when running OR when just completed (so user sees the summary)
    if !state.is_running && !state.completed {
        return None;
    }

    let mut result = None;
    let params = ModalParams {
        width_frac: 0.45,
        height_frac: 0.35,
        min: egui::vec2(420.0, 260.0),
        max: egui::vec2(600.0, 400.0),
        bottom_bar_height: 48.0,
        ..Default::default()
    };

    show_dimmed_modal(
        ctx,
        theme,
        "process_progress",
        &params,
        |ui, _rect| {
            if state.is_running {
                Text::new("Processing").size(18.0).strong().show(ui);
                ui.add_space(6.0);
                let file_line = format!(
                    "File {} of {}",
                    (state.files_done + 1).min(state.files_total.max(1)),
                    state.files_total.max(1),
                );
                Text::new(&file_line).show(ui);
                Text::new(&state.message).show(ui);
                ui.add_space(8.0);
                // The batch fraction, straight off the operation's own
                // completed/total units. The pre-facade bar showed a
                // per-step percentage instead; that number now arrives
                // only inside the human-readable message, and re-parsing
                // it back out would be a guess about the application's
                // wording rather than a reading of its data.
                let fraction = if state.files_total == 0 {
                    0.0
                } else {
                    state.files_done as f32 / state.files_total as f32
                };
                ui.add(egui::ProgressBar::new(fraction).show_percentage());
            } else {
                Text::new(if state.cancelled {
                    "Cancelled"
                } else {
                    "Complete"
                })
                .size(18.0)
                .strong()
                .show(ui);
                ui.add_space(6.0);
                if let Some(ref summary) = state.summary {
                    Text::new(summary).show(ui);
                }
            }

            if !state.log.is_empty() {
                ui.add_space(10.0);
                Text::new("Log").strong().show(ui);
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .id_salt("process_progress_log")
                    .max_height(180.0)
                    .stick_to_bottom(true)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for line in &state.log {
                            Text::new(line).size(11.0).monospace().show(ui);
                        }
                    });
            }
        },
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // One button, and it does what it says. The pre-facade
                // dialog offered "Minimize" while running, wired to the
                // same handler as "Close" -- which only cleared the
                // *completed* flag, so pressing it during a run did
                // nothing at all. There was no cancellation to offer
                // then; there is now, so the running state offers it
                // instead of an inert button.
                let (label, outcome) = if state.is_running {
                    ("Cancel", ProcessProgressResult::Cancel)
                } else {
                    ("Close", ProcessProgressResult::Close)
                };
                if ui
                    .add(TextButton::new(label, ButtonSize::Small).with_theme_colors(&theme.colors))
                    .clicked()
                {
                    result = Some(outcome);
                }
            });
        },
    );

    result
}
