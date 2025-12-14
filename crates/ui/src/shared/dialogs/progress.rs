use super::helpers::{show_dimmed_modal, ModalParams};
use crate::shared::theme::AppTheme;
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionStatus {
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct ExtractionProgressDialog {
    pub show: bool,
    pub title: String,
    pub file_action: String,
    pub percent: u8,
    pub processed_text: String,
    pub elapsed_text: String,
    pub time_left_text: String,
    pub status: ExtractionStatus,
    pub can_minimize: bool,
    pub can_pause: bool,
    pub can_cancel: bool,
    pub error: String,
    pub log_lines: Vec<String>,
    pub show_log: bool,
    /// Destination path for checksum verification
    pub dest_path: Option<std::path::PathBuf>,
}

impl Default for ExtractionProgressDialog {
    fn default() -> Self {
        Self {
            show: false,
            title: "Extracting".to_string(),
            file_action: String::new(),
            percent: 0,
            processed_text: String::new(),
            elapsed_text: String::new(),
            time_left_text: String::new(),
            status: ExtractionStatus::Running,
            can_minimize: true,
            can_pause: true,
            can_cancel: true,
            error: String::new(),
            log_lines: Vec::new(),
            show_log: true,
            dest_path: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractionDialogResult {
    None,
    Minimized,
    Paused,
    Resumed,
    Cancelled,
}

pub fn render_extraction_progress_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    dlg: &mut ExtractionProgressDialog,
) -> Option<ExtractionDialogResult> {
    if !dlg.show {
        return None;
    }

    let params = ModalParams {
        width_frac: 0.5,
        height_frac: 0.32,
        min: egui::vec2(520.0, 260.0),
        max: egui::vec2(900.0, 480.0),
        padding: egui::vec2(18.0, 14.0),
        bottom_bar_height: 56.0,
        ..Default::default()
    };

    let mut result: ExtractionDialogResult = ExtractionDialogResult::None;

    show_dimmed_modal(
        ctx,
        theme,
        "extraction_progress",
        &params,
        |ui, _rect| {
            // Header
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&dlg.title)
                        .size(16.0)
                        .color(theme.colors.on_surface)
                        .strong(),
                );
            });
            ui.add_space(8.0);

            // Current file/action row
            if !dlg.file_action.is_empty() {
                ui.label(
                    egui::RichText::new(&dlg.file_action)
                        .size(14.0)
                        .color(theme.colors.on_surface_variant),
                );
            }

            ui.add_space(10.0);

            // Big progress bar like modern UX
            let pct = dlg.percent as f32 / 100.0;
            let pb = egui::ProgressBar::new(pct)
                .desired_width(ui.available_width())
                .text(format!("{}%", dlg.percent))
                .fill(theme.colors.primary)
                .animate(true);
            ui.add(pb);

            ui.add_space(6.0);

            // Details row
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("Processed: {}", dlg.processed_text))
                        .color(theme.colors.on_surface_variant),
                );
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(format!("Elapsed: {}", dlg.elapsed_text))
                        .color(theme.colors.on_surface_variant),
                );
                ui.add_space(12.0);
                if !dlg.time_left_text.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("Time left: {}", dlg.time_left_text))
                            .color(theme.colors.on_surface_variant),
                    );
                }
            });

            ui.add_space(8.0);
            let header = if dlg.show_log {
                "▼ Details"
            } else {
                "▶ Details"
            };
            if ui
                .button(
                    egui::RichText::new(header)
                        .strong()
                        .color(theme.colors.on_surface_variant),
                )
                .clicked()
            {
                dlg.show_log = !dlg.show_log;
            }
            ui.add_space(4.0);

            if dlg.show_log {
                let frame = egui::Frame::new()
                    .fill(theme.colors.surface_variant)
                    .stroke(egui::Stroke::new(1.0, theme.colors.outline))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::same(8));
                frame.show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for line in &dlg.log_lines {
                                ui.label(
                                    egui::RichText::new(line)
                                        .color(theme.colors.on_surface_variant),
                                );
                            }
                        });
                });
            }

            if !dlg.error.is_empty() {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(&dlg.error).color(egui::Color32::RED));
            }
        },
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Cancel
                let cancel_enabled = dlg.can_cancel
                    && matches!(
                        dlg.status,
                        ExtractionStatus::Running | ExtractionStatus::Paused
                    );
                let cancel = egui::Button::new(egui::RichText::new("Cancel"))
                    .min_size(egui::vec2(100.0, 32.0));
                if ui.add_enabled(cancel_enabled, cancel).clicked() {
                    result = ExtractionDialogResult::Cancelled;
                }

                ui.add_space(8.0);

                // Pause/Resume
                let pause_enabled = dlg.can_pause
                    && matches!(
                        dlg.status,
                        ExtractionStatus::Running | ExtractionStatus::Paused
                    );
                let label = if dlg.status == ExtractionStatus::Paused {
                    "Resume"
                } else {
                    "Pause"
                };
                let pause_btn =
                    egui::Button::new(egui::RichText::new(label)).min_size(egui::vec2(100.0, 32.0));
                if ui.add_enabled(pause_enabled, pause_btn).clicked() {
                    result = if dlg.status == ExtractionStatus::Paused {
                        ExtractionDialogResult::Resumed
                    } else {
                        ExtractionDialogResult::Paused
                    };
                }

                ui.add_space(8.0);

                // Minimize (background)
                let minimize_enabled = dlg.can_minimize
                    && matches!(
                        dlg.status,
                        ExtractionStatus::Running | ExtractionStatus::Paused
                    );
                let minimize_btn = egui::Button::new(egui::RichText::new("Minimize"))
                    .min_size(egui::vec2(112.0, 32.0));
                if ui.add_enabled(minimize_enabled, minimize_btn).clicked() {
                    result = ExtractionDialogResult::Minimized;
                }
            });
        },
    );

    if result != ExtractionDialogResult::None {
        Some(result)
    } else {
        None
    }
}
