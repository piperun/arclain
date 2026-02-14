#![allow(unused)]
//! Plugin list view component

use crate::features::plugins::domain::types::{PluginInfo, PluginsListState};

use arclain_widgets::{ButtonSize, TextButton, TextInput};
use crate::shared::theme::AppTheme;
use eframe::egui;

/// Render the plugins list view
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut PluginsListState,
) -> Option<PluginAction> {
    let mut action = None;

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 8.0);

        // Header with search and controls
        egui::Frame::NONE
            .fill(theme.colors.surface_variant)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .corner_radius(8.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Search box
                    ui.label(egui::RichText::new("🔍").size(16.0));
                    ui.add_space(4.0);
                    let search_response = TextInput::new(&mut state.filter_text)
                        .hint("Search plugins...")
                        .width(200.0)
                        .with_theme_colors(&theme.colors)
                        .show(ui);

                    if search_response.changed() {
                        // Filter will be applied when rendering list
                    }

                    ui.add_space(8.0);

                    // Show disabled checkbox
                    ui.checkbox(&mut state.show_disabled, "Show disabled");

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Install plugin button
                        if ui
                            .add(TextButton::new("+ Install Plugin", ButtonSize::Medium).with_theme_colors(&theme.colors))
                            .clicked()
                        {
                            action = Some(PluginAction::InstallPlugin);
                        }
                    });
                });
            });

        ui.add_space(8.0);

        // Plugins list
        egui::ScrollArea::vertical()
            .id_salt("plugins_list_scroll")
            .show(ui, |ui| {
                let filtered_plugins: Vec<_> = state
                    .plugins
                    .iter()
                    .filter(|p| {
                        // Filter by search text
                        let matches_search = state.filter_text.is_empty()
                            || p.name
                                .to_lowercase()
                                .contains(&state.filter_text.to_lowercase())
                            || p.id
                                .to_lowercase()
                                .contains(&state.filter_text.to_lowercase())
                            || p.description
                                .as_ref()
                                .map(|d| {
                                    d.to_lowercase().contains(&state.filter_text.to_lowercase())
                                })
                                .unwrap_or(false);

                        // Filter by enabled status
                        let show = state.show_disabled || p.enabled;

                        matches_search && show
                    })
                    .collect();

                if filtered_plugins.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(40.0);
                        ui.label(
                            egui::RichText::new("No plugins found")
                                .size(16.0)
                                .color(theme.colors.on_surface_variant),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Install plugins to extend functionality")
                                .size(12.0)
                                .color(theme.colors.on_surface_variant),
                        );
                    });
                } else {
                    for plugin in filtered_plugins {
                        let plugin_action = render_plugin_card(ui, theme, plugin, state);
                        if plugin_action.is_some() {
                            action = plugin_action;
                        }
                    }
                }
            });
    });

    action
}

/// Render a single plugin card
fn render_plugin_card(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    plugin: &PluginInfo,
    state: &PluginsListState,
) -> Option<PluginAction> {
    let mut action = None;
    let is_selected = state.selected_plugin.as_ref() == Some(&plugin.id);

    let card = egui::Frame::NONE
        .fill(if is_selected {
            theme.colors.surface
        } else {
            theme.colors.surface_variant
        })
        .stroke(egui::Stroke::new(
            1.0,
            if is_selected {
                theme.colors.secondary
            } else {
                theme.colors.outline
            },
        ))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .corner_radius(8.0);

    let response = card
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Status indicator
                ui.label(
                    egui::RichText::new(plugin.status.icon())
                        .size(20.0)
                        .color(plugin.status.color()),
                );

                ui.add_space(8.0);

                // Plugin info
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);

                    // Name and version
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&plugin.name)
                                .size(14.0)
                                .strong()
                                .color(theme.colors.on_surface),
                        );

                        ui.label(
                            egui::RichText::new(format!("v{}", plugin.version))
                                .size(11.0)
                                .color(theme.colors.on_surface_variant),
                        );

                        // Enabled/Disabled badge
                        let badge_text = if plugin.enabled {
                            "Enabled"
                        } else {
                            "Disabled"
                        };
                        let badge_color = if plugin.enabled {
                            egui::Color32::from_rgb(100, 200, 100)
                        } else {
                            egui::Color32::GRAY
                        };

                        egui::Frame::NONE
                            .fill(badge_color.linear_multiply(0.2))
                            .inner_margin(egui::Margin::symmetric(6, 2))
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(badge_text)
                                        .size(10.0)
                                        .color(badge_color),
                                );
                            });
                    });

                    // Description
                    if let Some(desc) = &plugin.description {
                        ui.label(
                            egui::RichText::new(desc)
                                .size(12.0)
                                .color(theme.colors.on_surface_variant),
                        );
                    }

                    // Author
                    if let Some(author) = &plugin.author {
                        ui.label(
                            egui::RichText::new(format!("by {}", author))
                                .size(11.0)
                                .color(theme.colors.on_surface_variant),
                        );
                    }

                    // Capabilities
                    if !plugin.capabilities.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                            for cap in &plugin.capabilities {
                                egui::Frame::NONE
                                    .fill(theme.colors.outline)
                                    .inner_margin(egui::Margin::symmetric(4, 2))
                                    .corner_radius(3.0)
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(cap)
                                                .size(9.0)
                                                .color(theme.colors.on_surface_variant),
                                        );
                                    });
                            }
                        });
                    }

                    // Error message
                    if let Some(error) = &plugin.error {
                        ui.label(
                            egui::RichText::new(format!("⚠ {}", error))
                                .size(11.0)
                                .color(egui::Color32::from_rgb(255, 100, 100)),
                        );
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Enable/Disable toggle
                    let toggle_text = if plugin.enabled { "Disable" } else { "Enable" };
                    if ui.add(TextButton::new(toggle_text, ButtonSize::Small).with_theme_colors(&theme.colors)).clicked() {
                        action = Some(if plugin.enabled {
                            PluginAction::DisablePlugin(plugin.id.clone())
                        } else {
                            PluginAction::EnablePlugin(plugin.id.clone())
                        });
                    }

                    // Settings button
                    if ui.add(TextButton::new("⚙", ButtonSize::Small).with_theme_colors(&theme.colors)).clicked() {
                        action = Some(PluginAction::ShowPluginSettings(plugin.id.clone()));
                    }
                });
            });
        })
        .response;

    // Make card clickable for selection
    if response.interact(egui::Sense::click()).clicked() {
        action = Some(PluginAction::SelectPlugin(plugin.id.clone()));
    }

    action
}

/// Actions that can be triggered from the plugin list
#[derive(Debug, Clone)]
pub enum PluginAction {
    /// Select a plugin
    SelectPlugin(String),
    /// Enable a plugin
    EnablePlugin(String),
    /// Disable a plugin
    DisablePlugin(String),
    /// Show plugin settings
    ShowPluginSettings(String),
    /// Install a new plugin
    InstallPlugin,
}
