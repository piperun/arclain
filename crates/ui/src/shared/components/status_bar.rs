use crate::core::operations::window::format_duration;
use crate::shared::theme::AppTheme;
use eframe::egui;
use egui::Widget;
use std::time::Duration;

pub struct StatusBarInfo {
    pub message: String,
    pub file_count: usize,
    pub folder_count: usize,
    pub total_size: String,
    pub compressed_size: String,
    pub archive_format: String,
    pub active_operation: Option<String>,
    pub operation_time: Option<Duration>,
}

#[derive(Default)]
pub struct PluginStatusInfo {
    pub total_plugins: usize,
    pub enabled_plugins: usize,
    pub has_metadata: bool,
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
            active_operation: None,
            operation_time: None,
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
                .color(theme.colors.on_surface_variant),
        );

        // Plugin status indicator (if plugins are loaded)
        if let Some(pinfo) = plugin_info {
            if pinfo.total_plugins > 0 {
                ui.add_space(8.0);
                render_plugin_indicator(ui, theme, pinfo);
            }
        }

        // Active operation indicator
        if let Some(op) = &info.active_operation {
            ui.add_space(8.0);
            let label = if let Some(duration) = info.operation_time {
                format!("{} ({})", op, format_duration(duration))
            } else {
                op.clone()
            };
            arclain_widgets::Chips::new(&label)
                .with_theme_colors(&theme.colors)
                .ui(ui);
        }

        if archive_loaded {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(12.0);

                // Right side - archive info
                ui.label(
                    egui::RichText::new("Ready")
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );

                ui.label(
                    egui::RichText::new("|")
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );

                ui.label(
                    egui::RichText::new(&info.archive_format)
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );

                ui.label(
                    egui::RichText::new("|")
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );

                ui.label(
                    egui::RichText::new(format!(
                        "{} ({} compressed)",
                        info.total_size, info.compressed_size
                    ))
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
                );

                ui.label(
                    egui::RichText::new("|")
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );

                ui.label(
                    egui::RichText::new(format!("{} folders", info.folder_count))
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );

                ui.label(
                    egui::RichText::new("|")
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );

                ui.label(
                    egui::RichText::new(format!("{} files", info.file_count))
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
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
        format!(
            "{} {}/{} active",
            icon, info.enabled_plugins, info.total_plugins
        )
    };

    let color = if info.enabled_plugins > 0 {
        egui::Color32::from_rgb(76, 175, 80) // Green when plugins are active
    } else {
        theme.colors.on_surface_variant
    };

    let response = arclain_widgets::Chips::new(&text)
        .with_theme_colors(&theme.colors)
        .stroke_color(color)
        .ui(ui);

    response.on_hover_ui(|ui| {
        ui.label(egui::RichText::new("Plugin System").strong());
        ui.label(format!("Total plugins: {}", info.total_plugins));
        ui.label(format!("Enabled: {}", info.enabled_plugins));
        if info.has_metadata {
            ui.label(
                egui::RichText::new("✓ Metadata available")
                    .color(egui::Color32::from_rgb(76, 175, 80)),
            );
        }
        ui.label(
            egui::RichText::new("Click Settings → Plugins to manage")
                .size(10.0)
                .italics(),
        );
    });
}
