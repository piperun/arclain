use crate::core::operations::window::format_duration;
use crate::shared::theme::AppTheme;
use arclain_core::features::organization::GameMetadata;
use eframe::egui;
use egui::Widget;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
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
    selected_item: Option<&GameMetadata>,
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

        // "Item selected" chip — visible whenever a plugin has emitted
        // metadata for the current archive. Click to open a popup with
        // the full details. Both the Organizer and Process pages
        // observe this same metadata, so the chip serves as a "this is
        // what both pages will use" indicator.
        if let Some(meta) = selected_item {
            ui.add_space(8.0);
            render_selected_item_chip(ui, theme, meta);
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

/// Render the "Item selected" chip with a click-to-show popup of the
/// metadata fields that both the Organizer and Process pages will
/// consume.
fn render_selected_item_chip(ui: &mut egui::Ui, theme: &AppTheme, meta: &GameMetadata) {
    // Build a short label: title is the most informative, with the
    // product code as a fallback when the plugin only had the id.
    let label = if !meta.title.is_empty() {
        format!(
            "{} {}",
            egui_phosphor::regular::CHECK_CIRCLE,
            truncate_chars(&meta.title, 40)
        )
    } else {
        format!(
            "{} {}",
            egui_phosphor::regular::CHECK_CIRCLE,
            meta.product_id
        )
    };

    let response = arclain_widgets::Chips::new(&label)
        .with_theme_colors(&theme.colors)
        .ui(ui)
        .on_hover_text("Click to see what's selected");

    // Explicit toggle state stored in egui memory. Using the Popup
    // builder's auto-open-on-click together with a manual toggle_id
    // ended up double-handling the click and the popup wouldn't
    // close — easier to track the open bit ourselves.
    let memory_key = response.id.with("selected_item_open");
    let was_open = ui
        .data(|d| d.get_temp::<bool>(memory_key))
        .unwrap_or(false);
    let mut is_open = was_open;

    if response.clicked() {
        is_open = !is_open;
    }

    if is_open {
        let popup_id = response.id.with("selected_item_area");
        let popup_response = egui::Area::new(popup_id)
            .order(egui::Order::Foreground)
            // Anchor above the chip so the popup grows upward (status
            // bar is at the bottom of the window — opening downward
            // would clip).
            .fixed_pos(response.rect.left_top() - egui::vec2(0.0, 4.0))
            .pivot(egui::Align2::LEFT_BOTTOM)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(theme.colors.surface)
                    .stroke(egui::Stroke::new(1.0, theme.colors.outline))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_min_width(280.0);
                        ui.set_max_width(420.0);
                        ui.spacing_mut().item_spacing.y = 4.0;

                        arclain_widgets::Text::new("Selected for use")
                            .size(11.0)
                            .muted()
                            .show(ui);
                        ui.separator();

                        if !meta.title.is_empty() {
                            ui.label(
                                egui::RichText::new(&meta.title)
                                    .strong()
                                    .color(theme.colors.on_surface),
                            );
                        }
                        metadata_row(ui, theme, "ID", &meta.product_id);
                        if let Some(creator) = meta.creator.as_deref() {
                            metadata_row(ui, theme, "Creator", creator);
                        }
                        if let Some(date) = meta.release_date.as_deref() {
                            metadata_row(ui, theme, "Released", date);
                        }
                        if !meta.tags.is_empty() {
                            let preview = meta
                                .tags
                                .iter()
                                .take(6)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ");
                            let suffix = if meta.tags.len() > 6 {
                                format!(" (+{} more)", meta.tags.len() - 6)
                            } else {
                                String::new()
                            };
                            metadata_row(
                                ui,
                                theme,
                                "Tags",
                                &format!("{}{}", preview, suffix),
                            );
                        }

                        ui.add_space(4.0);
                        arclain_widgets::Text::new("Used by Organizer + Process")
                            .size(10.0)
                            .muted()
                            .show(ui);
                    });
            })
            .response;

        // Close on click outside the popup AND outside the chip (so
        // the toggle on the chip's own click still works).
        let clicked_anywhere = ui.input(|i| i.pointer.any_click());
        if clicked_anywhere
            && !popup_response.contains_pointer()
            && !response.contains_pointer()
        {
            is_open = false;
        }

        // Also close on Escape.
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            is_open = false;
        }
    }

    if is_open != was_open {
        ui.data_mut(|d| d.insert_temp(memory_key, is_open));
    }
}

fn metadata_row(ui: &mut egui::Ui, theme: &AppTheme, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{}:", label))
                .size(11.0)
                .color(theme.colors.on_surface_variant),
        );
        ui.label(
            egui::RichText::new(value)
                .size(11.0)
                .color(theme.colors.on_surface),
        );
    });
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max - 1).collect();
        format!("{}…", truncated)
    }
}

/// Render a small plugin status indicator using StatusIcon
fn render_plugin_indicator(ui: &mut egui::Ui, theme: &AppTheme, info: &PluginStatusInfo) {
    let icon = if info.has_metadata {
        egui_phosphor::regular::PLUGS_CONNECTED
    } else {
        egui_phosphor::regular::PLUG
    };

    let color = if info.enabled_plugins > 0 {
        theme.colors.success
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
