//! Modal progress dialog for pipeline runs.

use crate::core::signals::ProcessRunState;
use crate::shared::dialogs::helpers::{show_dimmed_modal, ModalParams};
use arclain_theme::AppTheme;
use arclain_widgets::{ButtonSize, Text, TextButton};
use eframe::egui;

pub fn render(
    ctx: &egui::Context,
    theme: &AppTheme,
    state: &ProcessRunState,
    close_requested: &mut bool,
) {
    // Only show when running OR when just completed (so user sees the summary)
    if !state.is_running && !state.completed {
        return;
    }

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
                    "File {} of {}: {}",
                    (state.files_done + 1).min(state.files_total.max(1)),
                    state.files_total.max(1),
                    state.current_file
                );
                Text::new(&file_line).show(ui);
                let step_line = format!("Step: {}", state.current_step);
                Text::new(&step_line).show(ui);
                ui.add_space(8.0);
                ui.add(
                    egui::ProgressBar::new(state.step_percent as f32 / 100.0)
                        .show_percentage(),
                );
                if state.files_failed > 0 {
                    ui.add_space(4.0);
                    let failed_line = format!("{} failed so far", state.files_failed);
                    Text::new(&failed_line).color(theme.colors.error).show(ui);
                }
            } else {
                // Completed
                Text::new("Complete").size(18.0).strong().show(ui);
                ui.add_space(6.0);
                let summary_suffix = if state.warnings.is_empty() {
                    String::new()
                } else {
                    format!(" — {} warning(s)", state.warnings.len())
                };
                if let Some(ref summary) = state.summary {
                    Text::new(&format!("{}{}", summary, summary_suffix)).show(ui);
                } else if !summary_suffix.is_empty() {
                    Text::new(summary_suffix.trim_start_matches(" — ")).show(ui);
                }
                if !state.warnings.is_empty() {
                    ui.add_space(10.0);
                    Text::new("Warnings").strong().show(ui);
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for w in &state.warnings {
                                Text::new(w).color(theme.colors.warning).show(ui);
                            }
                        });
                }
            }
        },
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let btn_label = if state.is_running { "Minimize" } else { "Close" };
                if ui
                    .add(
                        TextButton::new(btn_label, ButtonSize::Small)
                            .with_theme_colors(&theme.colors),
                    )
                    .clicked()
                {
                    *close_requested = true;
                }
            });
        },
    );
}
