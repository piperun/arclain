use super::theme::AppTheme;
use eframe::egui;

pub struct StatusBarInfo {
    pub message: String,
    pub file_count: usize,
    pub folder_count: usize,
    pub total_size: String,
    pub compressed_size: String,
    pub archive_format: String,
}

impl Default for StatusBarInfo {
    fn default() -> Self {
        Self {
            message: "Ready".to_string(),
            file_count: 0,
            folder_count: 0,
            total_size: String::new(),
            compressed_size: String::new(),
            archive_format: String::new(),
        }
    }
}

pub fn render(ui: &mut egui::Ui, theme: &AppTheme, info: &StatusBarInfo, archive_loaded: bool) {
    ui.horizontal(|ui| {
        ui.add_space(12.0);

        // Left side - status message
        ui.label(
            egui::RichText::new(&info.message)
                .size(12.0)
                .color(theme.colors.text_secondary),
        );

        if archive_loaded {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(12.0);

                // Right side - archive info
                ui.label(
                    egui::RichText::new("Ready")
                        .size(12.0)
                        .color(theme.colors.text_secondary),
                );

                ui.label(
                    egui::RichText::new("|")
                        .size(12.0)
                        .color(theme.colors.text_muted),
                );

                ui.label(
                    egui::RichText::new(&info.archive_format)
                        .size(12.0)
                        .color(theme.colors.text_secondary),
                );

                ui.label(
                    egui::RichText::new("|")
                        .size(12.0)
                        .color(theme.colors.text_muted),
                );

                ui.label(
                    egui::RichText::new(format!(
                        "{} ({} compressed)",
                        info.total_size, info.compressed_size
                    ))
                    .size(12.0)
                    .color(theme.colors.text_secondary),
                );

                ui.label(
                    egui::RichText::new("|")
                        .size(12.0)
                        .color(theme.colors.text_muted),
                );

                ui.label(
                    egui::RichText::new(format!("{} folders", info.folder_count))
                        .size(12.0)
                        .color(theme.colors.text_secondary),
                );

                ui.label(
                    egui::RichText::new("|")
                        .size(12.0)
                        .color(theme.colors.text_muted),
                );

                ui.label(
                    egui::RichText::new(format!("{} files", info.file_count))
                        .size(12.0)
                        .color(theme.colors.text_secondary),
                );
            });
        }
    });
}

// Lightweight pill button for background tasks
pub fn progress_chip(ui: &mut egui::Ui, theme: &AppTheme, label: &str) -> egui::Response {
    let frame = egui::Frame::none()
        .fill(theme.colors.bg_tertiary)
        .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
        .rounding(12.0)
        .inner_margin(egui::Margin::symmetric(10, 4));
    frame.show(ui, |ui| {
        ui.label(egui::RichText::new(label).size(12.0).color(theme.colors.text_primary));
    }).response
}
