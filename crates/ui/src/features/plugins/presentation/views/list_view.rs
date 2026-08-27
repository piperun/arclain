//! Plugin list view
//!
//! Renders the list of installed plugins with filtering and selection.

use crate::features::plugins::domain::types::{PluginStatus, PluginsListState};

use crate::shared::components::Form;
use crate::shared::theme::AppTheme;
use arclain_theme::spacing;
use arclain_widgets::Chips;
use eframe::egui;

/// Render the plugin list view
pub fn render(ui: &mut egui::Ui, theme: &AppTheme, state: &mut PluginsListState) {
    let filter_query = state.filter_text.clone();

    Form::new().show(ui, theme, |ui| {
        ui.add_space(8.0);

        for plugin in &state.plugins {
            // Filter logic
            if !filter_query.is_empty() {
                if !plugin
                    .name
                    .to_lowercase()
                    .contains(&filter_query.to_lowercase())
                    && !plugin
                        .id
                        .to_lowercase()
                        .contains(&filter_query.to_lowercase())
                {
                    continue;
                }
            }
            if !state.show_disabled && !plugin.enabled {
                continue;
            }

            // Card Item
            let response = render_plugin_card(ui, theme, plugin, state.show_permissions);

            if response.interact(egui::Sense::click()).clicked() {
                state.selected_plugin = Some(plugin.id.clone());
            }

            ui.add_space(4.0);
        }
    });
}

/// Render a single plugin card
fn render_plugin_card(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    plugin: &crate::features::plugins::domain::types::PluginInfo,
    show_permissions: bool,
) -> egui::Response {
    egui::Frame::NONE
        .fill(theme.colors.surface_variant.linear_multiply(0.3))
        .inner_margin(spacing::CARD)
        .corner_radius(6.0)
        .stroke(egui::Stroke::new(
            1.0_f32,
            theme.colors.outline.linear_multiply(0.2),
        ))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Status Icon
                render_status_icon(ui, theme, plugin);
                ui.add_space(8.0);

                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&plugin.name)
                            .strong()
                            .size(16.0)
                            .color(theme.colors.on_surface),
                    );
                    ui.label(
                        egui::RichText::new(plugin.description.as_deref().unwrap_or(""))
                            .color(theme.colors.on_surface_variant),
                    );

                    if show_permissions && !plugin.capabilities.is_empty() {
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                            for cap in &plugin.capabilities {
                                ui.add(Chips::new(cap));
                            }
                        });
                    }
                });

                // Right side info
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("v{}", plugin.version))
                            .small()
                            .color(theme.colors.on_surface_variant),
                    );
                });
            });
        })
        .response
}

/// Render the status icon for a plugin
fn render_status_icon(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    plugin: &crate::features::plugins::domain::types::PluginInfo,
) {
    let (status_rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
    if !ui.is_rect_visible(status_rect) {
        return;
    }

    let center = status_rect.center();

    if !plugin.enabled {
        // DISABLED State (Power Icon, Gray)
        let color = theme.colors.outline;
        ui.painter()
            .circle_stroke(center, 8.0, egui::Stroke::new(1.5_f32, color));
        ui.painter().text(
            center,
            egui::Align2::CENTER_CENTER,
            "⏻",
            egui::FontId::proportional(10.0),
            color,
        );
    } else {
        let status_color = plugin.status.color();

        match plugin.status {
            PluginStatus::Ready => {
                // Actively Running/Ready (Green Glow + Lightning)
                ui.painter()
                    .circle_filled(center, 10.0, status_color.linear_multiply(0.2));
                ui.painter()
                    .circle_stroke(center, 8.0, egui::Stroke::new(2.0_f32, status_color));
                ui.painter().text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    "⚡",
                    egui::FontId::proportional(12.0),
                    status_color,
                );
            }
            PluginStatus::Loading => {
                // Loading (Blue + Spinner)
                ui.painter()
                    .circle_stroke(center, 8.0, egui::Stroke::new(1.5_f32, status_color));
                ui.painter().text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    "⟳",
                    egui::FontId::proportional(10.0),
                    status_color,
                );
            }
            PluginStatus::Error => {
                // Error (Red + Warning)
                ui.painter()
                    .circle_filled(center, 8.0, status_color.linear_multiply(0.2));
                ui.painter()
                    .circle_stroke(center, 8.0, egui::Stroke::new(1.5_f32, status_color));
                ui.painter().text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    "⚠",
                    egui::FontId::proportional(10.0),
                    status_color,
                );
            }
            _ => {
                ui.painter()
                    .circle_stroke(center, 6.0, egui::Stroke::new(1.0_f32, status_color));
            }
        }
    }
}
