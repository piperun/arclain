//! Log Viewer Modal Dialog
//!
//! Displays network activity logs from plugins in a modal overlay.

use crate::shared::components::network_log::NetworkLog;
use crate::shared::dialogs::helpers::{show_dimmed_modal, ModalParams};
use arclain_theme::AppTheme;
use eframe::egui;
use std::time::SystemTime;

/// Render the log viewer modal
pub fn render(
    ctx: &egui::Context,
    theme: &AppTheme,
    logs: &[(SystemTime, String)],
    open: &mut bool,
) {
    if !*open {
        return;
    }

    let params = ModalParams {
        width_frac: 0.7,
        height_frac: 0.6,
        min: egui::vec2(500.0, 400.0),
        max: egui::vec2(1000.0, 700.0),
        bottom_bar_height: 48.0,
        ..Default::default()
    };

    show_dimmed_modal(
        ctx,
        theme,
        "log_viewer",
        &params,
        |ui, _content_rect| {
            ui.heading("Network Activity Logs");
            ui.add_space(8.0);

            if logs.is_empty() {
                ui.label(
                    egui::RichText::new("No network activity logged yet.")
                        .color(theme.colors.on_surface_variant),
                );
            } else {
                NetworkLog::render(ui, logs);
            }
        },
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    *open = false;
                }
            });
        },
    );
}
