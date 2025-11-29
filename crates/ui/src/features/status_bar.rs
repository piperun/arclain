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

pub struct PluginStatusInfo {
    pub total_plugins: usize,
    pub enabled_plugins: usize,
    pub has_metadata: bool,
}

impl Default for PluginStatusInfo {
    fn default() -> Self {
        Self {
            total_plugins: 0,
            enabled_plugins: 0,
            has_metadata: false,
        }
    }
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

pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    info: &StatusBarInfo,
    archive_loaded: bool,
    plugin_info: Option<&PluginStatusInfo>,
) {
    ui.horizontal(|ui| {
        ui.add_space(12.0);

        // Left side - status message
        ui.label(
            egui::RichText::new(&info.message)
                .size(12.0)
                .color(theme.colors.text_secondary),
        );

        // Plugin status indicator (if plugins are loaded)
        if let Some(pinfo) = plugin_info {
            if pinfo.total_plugins > 0 {
                ui.add_space(8.0);
                render_plugin_indicator(ui, theme, pinfo);
            }
        }

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

/// Render a small plugin status indicator
fn render_plugin_indicator(ui: &mut egui::Ui, theme: &AppTheme, info: &PluginStatusInfo) {
    let icon = if info.has_metadata { "🔌✓" } else { "🔌" };
    let text = if info.enabled_plugins == info.total_plugins {
        format!("{} {}/{}", icon, info.enabled_plugins, info.total_plugins)
    } else {
        format!("{} {}/{} active", icon, info.enabled_plugins, info.total_plugins)
    };
    
    let color = if info.enabled_plugins > 0 {
        egui::Color32::from_rgb(76, 175, 80) // Green when plugins are active
    } else {
        theme.colors.text_muted
    };

    let frame = egui::Frame::NONE
        .fill(theme.colors.bg_tertiary)
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(8, 2));
    
    let response = frame
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(11.0)
                    .color(color),
            );
        })
        .response;
    
    response.on_hover_ui(|ui| {
        ui.label(egui::RichText::new("Plugin System").strong());
        ui.label(format!("Total plugins: {}", info.total_plugins));
        ui.label(format!("Enabled: {}", info.enabled_plugins));
        if info.has_metadata {
            ui.label(egui::RichText::new("✓ Metadata available").color(egui::Color32::from_rgb(76, 175, 80)));
        }
        ui.label(egui::RichText::new("Click Settings → Plugins to manage").size(10.0).italics());
    });
}

// Lightweight pill button for background tasks
pub fn progress_chip(ui: &mut egui::Ui, theme: &AppTheme, label: &str) -> egui::Response {
    let frame = egui::Frame::NONE
        .fill(theme.colors.bg_tertiary)
        .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(10, 4));
    frame
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(label)
                    .size(12.0)
                    .color(theme.colors.text_primary),
            );
        })
        .response
}
