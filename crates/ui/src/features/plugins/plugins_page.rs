use crate::shared::theme::AppTheme;
use arclain_plugins::manager::PluginManager;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

use crate::features::plugins::types::PluginsListState;

/// Render the plugins page
///
/// Returns true if a plugin was interacted with (for triggering repaints)
pub fn render(
    ctx: &egui::Context,
    theme: &AppTheme,
    state: &mut PluginsListState,
    plugin_manager: Option<&Arc<Mutex<PluginManager>>>,
) -> bool {
    let mut needs_repaint = false;

    // Left sidebar - Plugin list
    egui::SidePanel::left("plugins_list")
        .exact_width(240.0)
        .frame(egui::Frame::NONE.fill(theme.colors.surface_variant))
        .show(ctx, |ui| {
            ui.add_space(8.0);

            ui.heading("Installed Plugins");
            ui.add_space(12.0);

            if let Some(manager_arc) = plugin_manager {
                let manager = manager_arc.lock();
                let plugins = manager.list_plugins();

                if plugins.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(32.0);
                        ui.label(
                            egui::RichText::new("No plugins installed")
                                .size(14.0)
                                .color(theme.colors.on_surface_variant),
                        );
                    });
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for plugin in plugins {
                            let is_selected = state
                                .selected_plugin
                                .as_ref()
                                .map(|id| id == &plugin.manifest.plugin.id)
                                .unwrap_or(false);

                            let frame = if is_selected {
                                egui::Frame::NONE
                                    .fill(theme.colors.surface_variant.linear_multiply(0.2))
                                    .inner_margin(egui::Margin::same(8))
                                    .corner_radius(4.0)
                            } else {
                                egui::Frame::NONE.inner_margin(egui::Margin::same(8))
                            };

                            let response = frame.show(ui, |ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(&plugin.manifest.plugin.name)
                                            .size(14.0)
                                            .color(theme.colors.on_surface),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "v{}",
                                            plugin.manifest.plugin.version
                                        ))
                                        .size(11.0)
                                        .color(theme.colors.on_surface_variant),
                                    );

                                    // Status indicator
                                    let status_text = if plugin.enabled {
                                        "✓ Enabled"
                                    } else {
                                        "○ Disabled"
                                    };
                                    let status_color = if plugin.enabled {
                                        egui::Color32::from_rgb(34, 197, 94)
                                    } else {
                                        theme.colors.on_surface_variant
                                    };
                                    ui.label(
                                        egui::RichText::new(status_text)
                                            .size(11.0)
                                            .color(status_color),
                                    );
                                });
                            });

                            if response.response.interact(egui::Sense::click()).clicked() {
                                state.selected_plugin = Some(plugin.manifest.plugin.id.clone());
                                needs_repaint = true;
                            }

                            ui.add_space(4.0);
                        }
                    });
                }
            } else {
                ui.label(
                    egui::RichText::new("Plugin system not initialized")
                        .color(theme.colors.on_surface_variant),
                );
            }
        });

    // Central panel - Plugin details
    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme.colors.surface)
                .inner_margin(16.0),
        )
        .show(ctx, |ui| {
            if let Some(selected_id) = &state.selected_plugin {
                if let Some(manager_arc) = plugin_manager {
                    let manager = manager_arc.lock();
                    if let Some(plugin) = manager
                        .list_plugins()
                        .iter()
                        .find(|p| &p.manifest.plugin.id == selected_id)
                    {
                        // Plugin header
                        ui.heading(&plugin.manifest.plugin.name);
                        ui.label(
                            egui::RichText::new(format!("by {}", plugin.manifest.plugin.author))
                                .size(13.0)
                                .color(theme.colors.on_surface_variant),
                        );
                        ui.add_space(8.0);
                        ui.label(&plugin.manifest.plugin.description);
                        ui.add_space(16.0);
                        ui.separator();
                        ui.add_space(16.0);

                        // Plugin UI (if provided)
                        ui.label(egui::RichText::new("Settings").size(16.0).strong());
                        ui.add_space(8.0);

                        // Drop the manager lock before accessing plugin instance
                        drop(manager);

                        // Get plugin UI layout for MainPage
                        let ui_result =
                            manager_arc
                                .lock()
                                .with_plugin_instance(selected_id, |instance| {
                                    instance.get_ui_layout(
                                        arclain_plugins::types::PluginExtensionPoint::MainPage,
                                    )
                                });

                        if let Some(Ok(ui_elements)) = ui_result {
                            if ui_elements.is_empty() {
                                ui.label(
                                    egui::RichText::new(
                                        "This plugin does not provide a settings UI.",
                                    )
                                    .color(theme.colors.on_surface_variant),
                                );
                            } else {
                                // Create event callback
                                let plugin_id_clone = selected_id.clone();
                                let manager_clone = manager_arc.clone();
                                let mut event_callback =
                                    Box::new(move |id: &str, value: Option<String>| {
                                        manager_clone.lock().with_plugin_instance(
                                            &plugin_id_clone,
                                            |instance| {
                                                let _ = instance.send_ui_event(id, value);
                                            },
                                        );
                                    })
                                        as crate::features::plugins::plugin_ui::UiEventCallback;

                                // Render the UI elements
                                crate::features::plugins::plugin_ui::render_ui_elements(
                                    ui,
                                    &ui_elements,
                                    &mut event_callback,
                                );
                                needs_repaint = true;
                            }
                        } else {
                            ui.label(
                                egui::RichText::new("Failed to load plugin UI.")
                                    .color(egui::Color32::from_rgb(239, 68, 68)),
                            );
                        }
                    }
                }
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(100.0);
                    ui.label(
                        egui::RichText::new("Select a plugin from the list")
                            .size(16.0)
                            .color(theme.colors.on_surface_variant),
                    );
                });
            }
        });

    needs_repaint
}
