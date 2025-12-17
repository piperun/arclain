//! Plugin detail view
//!
//! Renders plugin settings, permissions, and custom UI when a plugin is selected.

use crate::features::plugins::types::PluginsListState;
use crate::shared::components::SettingsForm;
use crate::shared::theme::AppTheme;
use arclain_plugins::PluginManager;
use arclain_widgets::toggle_switch::ToggleSwitch;
use arclain_widgets::Chips;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

/// Render the plugin detail view
/// Returns true if the plugin list needs to be refreshed
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    plugin_manager: Option<&PluginManager>,
    state: &mut PluginsListState,
    app_state: &Arc<Mutex<crate::core::AppState>>,
) -> bool {
    let mut needs_refresh = false;

    let selected_id = match &state.selected_plugin {
        Some(id) => id.clone(),
        None => return false,
    };

    let plugin_info = match state.plugins.iter().find(|p| p.id == selected_id) {
        Some(info) => info.clone(),
        None => {
            // ID not found, reset
            state.selected_plugin = None;
            return false;
        }
    };

    SettingsForm::new().show(ui, theme, |ui| {
        // Global Settings
        crate::shared::components::settings_form::SectionHeader::new("Global Settings")
            .show(ui, &theme.colors);

        // Enabled/Disabled Toggle using SettingsRow
        let mut enabled = plugin_info.enabled;
        crate::shared::components::settings_form::SettingsRow::new("Plugin Status")
            .description("Enable or disable this plugin completely.")
            .action(|ui| {
                if ui
                    .add(ToggleSwitch::new(&mut enabled).icons(
                        egui_phosphor::regular::LIGHTNING,
                        egui_phosphor::regular::POWER,
                    ))
                    .changed()
                {
                    if let Some(mgr) = plugin_manager {
                        let res = if enabled {
                            mgr.enable_plugin(&plugin_info.id)
                        } else {
                            mgr.disable_plugin(&plugin_info.id)
                        };

                        if res.is_ok() {
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
            })
            .show(ui, &theme.colors);

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // Permissions / Capabilities
        crate::shared::components::settings_form::SectionHeader::new("Permissions")
            .show(ui, &theme.colors);
        if plugin_info.capabilities.is_empty() {
            ui.label(
                egui::RichText::new("None declared")
                    .italics()
                    .color(theme.colors.on_surface_variant),
            );
        } else {
            ui.horizontal_wrapped(|ui| {
                for cap in &plugin_info.capabilities {
                    ui.add(Chips::new(cap));
                }
            });
        }

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(16.0);

        // Plugin Custom Settings
        crate::shared::components::settings_form::SectionHeader::new("Plugin Configuration")
            .show(ui, &theme.colors);

        if let Some(manager) = plugin_manager {
            if plugin_info.loaded {
                render_plugin_ui(ui, theme, manager, &plugin_info.id, app_state);
            } else {
                ui.label(
                    egui::RichText::new("Plugin is not loaded.")
                        .color(theme.colors.on_surface_variant),
                );
            }
        }
    });

    needs_refresh
}

/// Render the plugin's custom UI elements
fn render_plugin_ui(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    manager: &PluginManager,
    plugin_id: &str,
    app_state: &Arc<Mutex<crate::core::AppState>>,
) {
    let mgr_arc = if let Some(mgr_mutex) = app_state.lock().plugin_manager.clone() {
        mgr_mutex
    } else {
        return;
    };

    let ui_result = if let Some(instance_arc) = manager.get_plugin_instance(plugin_id) {
        if let Some(mut instance) = instance_arc.try_lock() {
            Some(instance.get_ui_layout(arclain_plugins::types::PluginExtensionPoint::MainPage))
        } else {
            None // Busy
        }
    } else {
        Some(Err(arclain_plugins::types::PluginError::NotFound(
            plugin_id.to_string(),
        )))
    };

    match ui_result {
        Some(Ok(ui_elements)) => {
            if ui_elements.is_empty() {
                ui.label(
                    egui::RichText::new("This plugin does not provide configuration.")
                        .color(theme.colors.on_surface_variant),
                );
            } else {
                let plugin_id_clone = plugin_id.to_string();
                let mgr_clone = mgr_arc.clone();
                let app_state_clone = app_state.clone();

                let mut event_callback = Box::new(move |id: &str, value: Option<String>| {
                    let mgr_thread = mgr_clone.clone();
                    let pid_thread = plugin_id_clone.clone();
                    let id_thread = id.to_string();
                    let val_thread = value.clone();
                    let state_thread = app_state_clone.clone();

                    std::thread::spawn(move || {
                        let settings_to_save = {
                            let mgr = mgr_thread.lock();
                            if let Some(instance_arc) = mgr.get_plugin_instance(&pid_thread) {
                                let mut instance = instance_arc.lock();
                                let _ = instance.send_ui_event(&id_thread, val_thread);
                            }
                            mgr.get_all_settings()
                        };

                        let mut state = state_thread.lock();
                        state.user_config.set_all_plugin_settings(&settings_to_save);

                        if let Some(ref dbs) = state.dbs {
                            if let Err(e) = dbs.config.with_connection(|conn| {
                                state.user_config.save(conn)?;
                                Ok(())
                            }) {
                                eprintln!("Failed to save plugin settings: {}", e);
                            }
                        }
                    });
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
        Some(Err(e)) => {
            ui.label(
                egui::RichText::new(format!("Error loading UI: {}", e)).color(theme.colors.error),
            );
        }
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    egui::RichText::new("Plugin is busy...")
                        .italics()
                        .color(theme.colors.on_surface_variant),
                );
            });
        }
    }
}
