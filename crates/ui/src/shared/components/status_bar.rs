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
    ui.horizontal_centered(|ui| {
        ui.add_space(12.0);

        // Left side - status message
        arclain_widgets::Text::new(&info.message)
            .size(12.0)
            .muted()
            .show(ui);

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

                // Right side - archive info (rendered in reverse order due to RTL)
                arclain_widgets::Text::new("Ready")
                    .size(12.0)
                    .muted()
                    .show(ui);

                arclain_widgets::Text::new("|").size(12.0).muted().show(ui);

                arclain_widgets::Text::new(&info.archive_format)
                    .size(12.0)
                    .muted()
                    .show(ui);

                arclain_widgets::Text::new("|").size(12.0).muted().show(ui);

                arclain_widgets::Text::new(&format!(
                    "{} ({} compressed)",
                    info.total_size, info.compressed_size
                ))
                .size(12.0)
                .muted()
                .show(ui);

                arclain_widgets::Text::new("|").size(12.0).muted().show(ui);

                arclain_widgets::Text::new(&format!("{} folders", info.folder_count))
                    .size(12.0)
                    .muted()
                    .show(ui);

                arclain_widgets::Text::new("|").size(12.0).muted().show(ui);

                arclain_widgets::Text::new(&format!("{} files", info.file_count))
                    .size(12.0)
                    .muted()
                    .show(ui);
            });
        }
    });
}

/// Render a small plugin status indicator using StatusIcon
fn render_plugin_indicator(ui: &mut egui::Ui, theme: &AppTheme, info: &PluginStatusInfo) {
    let icon = if info.has_metadata {
        egui_phosphor::regular::PLUGS_CONNECTED
    } else {
        egui_phosphor::regular::PLUG
    };

    let color = if info.enabled_plugins > 0 {
        egui::Color32::from_rgb(76, 175, 80) // Green when plugins are active
    } else {
        theme.colors.on_surface_variant
    };

    let tooltip = if info.has_metadata {
        "Plugins active with metadata"
    } else {
        "Plugins active"
    };

    super::status_icon::StatusIcon::new(icon)
        .count(info.enabled_plugins, info.total_plugins)
        .color(color)
        .tooltip(tooltip)
        .show(ui, theme);
}
