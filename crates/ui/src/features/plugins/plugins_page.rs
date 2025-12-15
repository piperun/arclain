//! Unified Plugin Page
//!
//! Master-detail view for managing plugins, their settings, and visibility.

use crate::features::plugins::types::PluginsListState;
use crate::features::settings::types::SettingsAction;
use crate::shared::theme::AppTheme;
use arclain_plugins::PluginManager;
use arclain_widgets::toggle_switch::ToggleSwitch;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

/// Render the updated Plugin Page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    plugin_manager: Option<&PluginManager>,
    state: &mut PluginsListState,
    app_state: &Arc<Mutex<crate::core::AppState>>,
) -> Option<SettingsAction> {
    let mut action = None;
    let mut needs_refresh = false;

    // Split into Left (List) and Right (Details)
    // We use a predefined width for the list
    let list_width = 300.0;

    egui::SidePanel::left("unified_plugins_list")
        .resizable(true)
        .default_width(list_width)
        .min_width(250.0)
        .max_width(400.0)
        .frame(egui::Frame::NONE.fill(theme.colors.surface_variant))
        .show_inside(ui, |ui| {
            // --- Left Panel Content ---
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Plugins")
                        .strong()
                        .size(16.0)
                        .color(theme.colors.on_surface),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(egui::RichText::new("+").size(16.0))
                        .on_hover_text("Install Plugin")
                        .clicked()
                    {
                        if let Some(file) = rfd::FileDialog::new()
                            .add_filter("WASM Plugin", &["wasm"])
                            .set_title("Select Plugin to Install")
                            .pick_file()
                        {
                            action = Some(SettingsAction::InstallPlugin {
                                wasm_path: file.to_string_lossy().to_string(),
                            });
                        }
                    }
                });
            });
            ui.add_space(8.0);

            // Search
            ui.horizontal(|ui| {
                ui.label("🔍");
                ui.add(
                    egui::TextEdit::singleline(&mut state.filter_text)
                        .hint_text("Search...")
                        .desired_width(ui.available_width()),
                );
            });
            ui.add_space(4.0);
            ui.checkbox(&mut state.show_disabled, "Show Disabled");
            ui.add_space(8.0);
            ui.separator();

            // List
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);

                // We use simple iteration now

                for plugin in &state.plugins {
                    // Filter logic
                    if !state.filter_text.is_empty() {
                        if !plugin
                            .name
                            .to_lowercase()
                            .contains(&state.filter_text.to_lowercase())
                            && !plugin
                                .id
                                .to_lowercase()
                                .contains(&state.filter_text.to_lowercase())
                        {
                            continue;
                        }
                    }
                    if !state.show_disabled && !plugin.enabled {
                        continue;
                    }

                    let is_selected = state.selected_plugin.as_ref() == Some(&plugin.id);

                    let bg = if is_selected {
                        theme.colors.surface_variant.linear_multiply(0.5) // Darker variant
                    } else {
                        egui::Color32::TRANSPARENT
                    };

                    let response = egui::Frame::NONE
                        .fill(bg)
                        .inner_margin(8.0)
                        .corner_radius(4.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                // Status Dot
                                ui.label(
                                    egui::RichText::new(plugin.status.icon())
                                        .color(plugin.status.color())
                                        .size(12.0),
                                );

                                // Name & Version
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(&plugin.name)
                                            .strong()
                                            .color(theme.colors.on_surface),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!("v{}", plugin.version))
                                            .small()
                                            .color(theme.colors.on_surface_variant),
                                    );
                                });
                            });
                        })
                        .response;

                    if response.interact(egui::Sense::click()).clicked() {
                        state.selected_plugin = Some(plugin.id.clone());
                    }
                }
            });
        });

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme.colors.surface)
                .inner_margin(24.0),
        )
        .show_inside(ui, |ui| {
            if let Some(selected_id) = &state.selected_plugin {
                // Find selected plugin into
                if let Some(plugin_info) = state.plugins.iter().find(|p| &p.id == selected_id) {
                    // --- Detail View ---

                    // Header
                    ui.horizontal(|ui| {
                        ui.heading(&plugin_info.name);
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(format!("v{}", plugin_info.version))
                                .size(14.0)
                                .color(theme.colors.on_surface_variant),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "by {}",
                                    plugin_info.author.as_deref().unwrap_or("Unknown")
                                ))
                                .color(theme.colors.on_surface_variant),
                            );
                        });
                    });

                    if let Some(desc) = &plugin_info.description {
                        ui.label(egui::RichText::new(desc).color(theme.colors.on_surface_variant));
                    }
                    ui.add_space(16.0);
                    ui.separator();
                    ui.add_space(16.0);

                    // Global Settings
                    ui.label(egui::RichText::new("Global Settings").strong().size(14.0));
                    ui.add_space(8.0);

                    // Enabled/Disabled Toggle
                    ui.horizontal(|ui| {
                        let mut enabled = plugin_info.enabled;
                        if ui
                            .add(ToggleSwitch::new(&mut enabled).text("Enabled", "Disabled"))
                            .changed()
                        {
                            // Handle toggle
                            if let Some(mgr) = plugin_manager {
                                let res = if enabled {
                                    mgr.enable_plugin(&plugin_info.id)
                                } else {
                                    mgr.disable_plugin(&plugin_info.id)
                                };

                                if res.is_ok() {
                                    // Persist enabled state
                                    // Update UserConfig.enabled_plugins
                                    let mut app = app_state.lock();
                                    let mut enabled_list = app.user_config.get_enabled_plugins();
                                    if enabled {
                                        if !enabled_list.contains(&plugin_info.id) {
                                            enabled_list.push(plugin_info.id.clone());
                                        }
                                    } else {
                                        enabled_list.retain(|id| id != &plugin_info.id);
                                    }
                                    app.user_config.set_enabled_plugins(&enabled_list);

                                    // Save DB
                                    if let Some(dbs) = &app.dbs {
                                        let _ = dbs.config.with_connection(|conn| {
                                            app.user_config.save(conn).ok();
                                            Ok::<_, anyhow::Error>(())
                                        });
                                    }
                                    needs_refresh = true;
                                }
                            }
                        }
                        ui.label("Enable or disable this plugin completely.");
                    });
                    ui.add_space(8.0);

                    // Visibility Settings
                    let mut visibility = plugin_info.visibility.clone();
                    let mut vis_changed = false;

                    ui.horizontal(|ui| {
                        let mut toolbar = visibility.get("toolbar").copied().unwrap_or(true);
                        if ui.checkbox(&mut toolbar, "Show in Toolbar").changed() {
                            visibility.insert("toolbar".to_string(), toolbar);
                            vis_changed = true;
                        }

                        ui.add_space(16.0);

                        let mut info_panel = visibility.get("info_panel").copied().unwrap_or(true);
                        if ui.checkbox(&mut info_panel, "Show in Info Panel").changed() {
                            visibility.insert("info_panel".to_string(), info_panel);
                            vis_changed = true;
                        }
                    });

                    if vis_changed {
                        // Update UserConfig
                        let mut app = app_state.lock();
                        let mut vis_map: std::collections::HashMap<
                            String,
                            std::collections::HashMap<String, bool>,
                        > = serde_json::from_str(
                            app.user_config.plugin_visibility.as_deref().unwrap_or("{}"),
                        )
                        .unwrap_or_default();

                        vis_map.insert(plugin_info.id.clone(), visibility);

                        // Serialize back
                        if let Ok(json) = serde_json::to_string(&vis_map) {
                            app.user_config.plugin_visibility = Some(json);

                            // Save DB
                            if let Some(dbs) = &app.dbs {
                                let _ = dbs.config.with_connection(|conn| {
                                    app.user_config.save(conn).ok();
                                    Ok::<_, anyhow::Error>(())
                                });
                            }
                            needs_refresh = true;
                        }
                    }

                    ui.add_space(16.0);
                    ui.separator();
                    ui.add_space(16.0);

                    // Privileges / Capabilities
                    ui.label(egui::RichText::new("Privileges").strong().size(14.0));
                    ui.add_space(4.0);
                    if plugin_info.capabilities.is_empty() {
                        ui.label(
                            egui::RichText::new("None declared")
                                .italics()
                                .color(theme.colors.on_surface_variant),
                        );
                    } else {
                        ui.horizontal_wrapped(|ui| {
                            for cap in &plugin_info.capabilities {
                                ui.add(
                                    arclain_widgets::chip::Chip::new(cap)
                                        .stroke_color(theme.colors.outline),
                                );
                            }
                        });
                    }

                    ui.add_space(16.0);
                    ui.separator();
                    ui.add_space(16.0);

                    // Plugin Custom Settings
                    ui.label(
                        egui::RichText::new("Plugin Configuration")
                            .strong()
                            .size(16.0),
                    );
                    ui.add_space(8.0);

                    if let Some(manager) = plugin_manager {
                        // Only show if plugin is enabled/loaded
                        if plugin_info.loaded {
                            // Render Plugin UI
                            // We need to drop the manager lock before accessing plugin instance?
                            // Wait, plugin_manager is &PluginManager (not locked).
                            // But inside `with_plugin_instance` it locks.

                            // Reuse logic from old `render`
                            let mgr_arc =
                                if let Some(mgr_mutex) = app_state.lock().plugin_manager.clone() {
                                    mgr_mutex
                                } else {
                                    // This is awkward. plugin_manager passed in is `&PluginManager`.
                                    // To call `with_plugin_instance`, we need interior mutability or just `&self`.
                                    // `PluginManager` methods take `&self` and use internal RwLock.
                                    // But `render_ui_elements` needs callback.

                                    // Actually, `PluginManager::with_plugin_instance` takes `&self`.
                                    // But to get the *Arc* for the callback, we need access to the Arc.
                                    // `app_state` has `plugin_manager: Option<Arc<Mutex<PluginManager>>>`.
                                    return; // Should not happen if we are here
                                };

                            // Wait, `app_state.plugin_manager` is `Arc<Mutex<PluginManager>>`.
                            // But the passed `plugin_manager` is `&PluginManager`.
                            // We should use the Arc from app_state for the callback.

                            let ui_result =
                                manager.with_plugin_instance(&plugin_info.id, |instance| {
                                    instance.get_ui_layout(
                                        arclain_plugins::types::PluginExtensionPoint::MainPage,
                                    )
                                });

                            if let Some(Ok(ui_elements)) = ui_result {
                                if ui_elements.is_empty() {
                                    ui.label(
                                        egui::RichText::new(
                                            "This plugin does not provide configuration.",
                                        )
                                        .color(theme.colors.on_surface_variant),
                                    );
                                } else {
                                    let plugin_id_clone = plugin_info.id.clone();
                                    let mgr_clone = mgr_arc.clone();

                                    // Create separate callback
                                    let mut event_callback =
                                        Box::new(move |id: &str, value: Option<String>| {
                                            let mgr = mgr_clone.lock();
                                            mgr.with_plugin_instance(
                                                &plugin_id_clone,
                                                |instance| {
                                                    let _ = instance.send_ui_event(id, value);
                                                },
                                            );
                                        })
                                            as crate::features::plugins::plugin_ui::UiEventCallback;

                                    crate::features::plugins::plugin_ui::render_ui_elements(
                                        ui,
                                        &ui_elements,
                                        &mut event_callback,
                                        &theme.colors,
                                    );
                                }
                            }
                        } else {
                            ui.label(
                                egui::RichText::new("Plugin is not loaded.")
                                    .color(theme.colors.on_surface_variant),
                            );
                        }
                    }
                } else {
                    ui.label("Selected plugin not found.");
                }
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(100.0);
                    ui.label(
                        egui::RichText::new("Select a plugin to configure")
                            .size(16.0)
                            .color(theme.colors.on_surface_variant),
                    );
                });
            }
        });

    if needs_refresh {
        if let Some(manager) = plugin_manager {
            let state_lock = app_state.lock();
            state.update_from_manager(manager, &state_lock.user_config);
        }
    }

    action
}
