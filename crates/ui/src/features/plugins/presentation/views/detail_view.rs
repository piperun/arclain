//! Plugin detail view
//!
//! Renders plugin settings, permissions, and custom UI when a plugin is selected.

use crate::features::plugins::domain::types::PluginsListState;

use crate::features::plugins::presentation::rendering as ui;

use crate::shared::components::SettingsForm;
use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
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
    shared: Option<&SharedState>,
    content_cache: Option<&Arc<arclain_data::ContentCache>>,
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

    // Fetch whitelist entries for this plugin
    let whitelist_entries = if let Some(shared) = shared {
        let whitelist = shared.services.domain_whitelist.read();
        whitelist
            .get_all_entries()
            .into_iter()
            .filter(|e| e.plugin_id == selected_id)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    // Capture config_service for use in closures
    let config_service: Option<Arc<arclain_core::ConfigService>> = if let Some(s) = shared {
        s.services.config_service.clone()
    } else {
        None
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

                            if let Some(ref cfg_svc) = config_service {
                                let res: anyhow::Result<()> =
                                    cfg_svc.save_user_config(&app.user_config);
                                if let Err(e) = res {
                                    eprintln!("Failed to save user config: {}", e);
                                }
                            }
                            needs_refresh = true;
                        }
                    }
                }
            })
            .show(ui, &theme.colors);

        // Proxy Settings
        let proxy_enabled = {
            let app = app_state.lock();
            app.user_config
                .get_plugin_proxy_settings()
                .get(&plugin_info.id)
                .cloned()
                .unwrap_or(false)
        };
        let mut proxy_toggle_val = proxy_enabled;

        // Extract http client from services for use in closure
        let http_client = shared.map(|s| s.services.async_http_client.clone());

        crate::shared::components::settings_form::SettingsRow::new("Network Proxy")
            .description("Route this plugin's traffic through the configured SOCKS5 proxy.")
            .action(|ui| {
                if ui.add(ToggleSwitch::new(&mut proxy_toggle_val)).changed() {
                    let mut app = app_state.lock();
                    app.user_config
                        .set_plugin_proxy_enabled(&plugin_info.id, proxy_toggle_val);

                    if let Some(ref cfg_svc) = config_service {
                        let res: anyhow::Result<()> = cfg_svc.save_user_config(&app.user_config);
                        if let Err(e) = res {
                            eprintln!("Failed to save user config: {}", e);
                        }
                    }

                    // Update Client from services
                    let map = app.user_config.get_plugin_proxy_settings();
                    if let Some(client) = &http_client {
                        client.update_plugin_proxy_map(map);
                    }

                    needs_refresh = true;
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

        // Domain Access
        crate::shared::components::settings_form::SectionHeader::new("Domain Access")
            .show(ui, &theme.colors);

        if whitelist_entries.is_empty() {
            ui.label(
                egui::RichText::new("No network domains requested.")
                    .italics()
                    .color(theme.colors.on_surface_variant),
            );
        } else {
            for entry in &whitelist_entries {
                if render_domain_row(ui, theme, entry, shared) {
                    needs_refresh = true;
                }
                ui.add_space(8.0);
            }
        }

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(16.0);

        // Plugin Custom Settings
        crate::shared::components::settings_form::SectionHeader::new("Plugin Configuration")
            .show(ui, &theme.colors);

        if let Some(manager) = plugin_manager {
            if plugin_info.loaded {
                render_plugin_ui(
                    ui,
                    theme,
                    manager,
                    &plugin_info.id,
                    app_state,
                    shared,
                    content_cache,
                );
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

fn render_domain_row(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entry: &arclain_http::features::whitelist::WhitelistEntry,
    shared: Option<&SharedState>,
) -> bool {
    let mut changed = false;
    let domain = &entry.domain;
    let is_approved = entry.approved;

    ui.horizontal(|ui| {
        // Status Icon
        if is_approved {
            ui.label(
                egui::RichText::new(egui_phosphor::regular::CHECK_CIRCLE)
                    .color(theme.colors.success)
                    .size(16.0),
            );
        } else {
            ui.label(
                egui::RichText::new(egui_phosphor::regular::WARNING)
                    .color(theme.colors.warning)
                    .size(16.0),
            );
        }

        ui.vertical(|ui| {
            ui.label(egui::RichText::new(domain).strong());

            // Security Analysis
            let url_for_check = format!("https://{}", domain);
            if let Ok(info) = arclain_http::features::security::analyze_url(&url_for_check) {
                if !info.warnings.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for warning in info.warnings {
                            ui.label(
                                egui::RichText::new(format!("⚠ {}", warning.description()))
                                    .small()
                                    .color(theme.colors.error),
                            );
                        }
                    });
                }
            }
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut approved_state = is_approved;
            if ui.add(ToggleSwitch::new(&mut approved_state)).changed() {
                // Update in-memory whitelist via shared.services
                if let Some(shared) = shared {
                    if approved_state {
                        shared
                            .services
                            .domain_whitelist
                            .write()
                            .approve(&entry.plugin_id, domain);
                    } else {
                        shared
                            .services
                            .domain_whitelist
                            .write()
                            .revoke(&entry.plugin_id, domain);
                    }
                }

                // Update DB via ConfigService
                if let Some(shared) = shared {
                    if let Some(config_svc) = shared.services.config_service.as_ref() {
                        if approved_state {
                            let _ = config_svc.approve_plugin_domain(&entry.plugin_id, domain);
                        } else {
                            let _ = config_svc.revoke_plugin_domain(&entry.plugin_id, domain);
                        }
                    }
                }
                changed = true;
            }
        });
    });

    changed
}

/// Render the plugin's custom UI elements
fn render_plugin_ui(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    manager: &PluginManager,
    plugin_id: &str,
    app_state: &Arc<Mutex<crate::core::AppState>>,
    shared: Option<&SharedState>,
    content_cache: Option<&Arc<arclain_data::ContentCache>>,
) {
    let mgr_arc = if let Some(shared) = shared {
        if let Some(mgr_mutex) = shared.services.plugin_manager.clone() {
            mgr_mutex
        } else {
            return;
        }
    } else {
        // No shared state means no plugin_manager available
        return;
    };

    // Capture config_service for use in closures
    let config_service: Option<Arc<arclain_core::ConfigService>> = if let Some(s) = shared {
        s.services.config_service.clone()
    } else {
        None
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
                let config_service_clone: Option<Arc<arclain_core::ConfigService>> =
                    config_service.clone();

                // Collect actions for processing after callback
                let collected_actions: Arc<Mutex<Vec<arclain_plugins::types::PluginAction>>> =
                    Arc::new(Mutex::new(Vec::new()));
                let actions_sink = collected_actions.clone();

                let mut event_callback = Box::new(move |id: &str, value: Option<String>| {
                    let mgr_thread = mgr_clone.clone();
                    let pid_thread = plugin_id_clone.clone();
                    let id_thread = id.to_string();
                    let val_thread = value.clone();
                    let state_thread = app_state_clone.clone();
                    let sink = actions_sink.clone();
                    let cfg_svc_thread: Option<Arc<arclain_core::ConfigService>> =
                        config_service_clone.clone();

                    std::thread::spawn(move || {
                        let (settings_to_save, actions) = {
                            let mgr = mgr_thread.lock();
                            if let Some(instance_arc) = mgr.get_plugin_instance(&pid_thread) {
                                let mut instance = instance_arc.lock();
                                let actions = instance.send_ui_event(&id_thread, val_thread).ok();
                                (Some(mgr.get_all_settings()), actions)
                            } else {
                                (None, None)
                            }
                        };

                        // Collect actions for later processing (though async)
                        if let Some(actions) = actions {
                            let mut s = sink.lock();
                            for a in actions {
                                s.push(a);
                            }
                        }

                        if let Some(settings_to_save) = settings_to_save {
                            let mut state = state_thread.lock();
                            state.user_config.set_all_plugin_settings(&settings_to_save);

                            if let Some(ref cfg_svc) = cfg_svc_thread {
                                let res: anyhow::Result<()> =
                                    cfg_svc.save_user_config(&state.user_config);
                                if let Err(e) = res {
                                    eprintln!("Failed to save plugin settings: {}", e);
                                }
                            }
                        }
                    });
                }) as ui::UiEventCallback;

                let flat_elements = ui_elements.flatten();
                ui::render_ui_elements(
                    ui,
                    &flat_elements,
                    &mut event_callback,
                    &theme.colors,
                    content_cache,
                    shared,
                    Some(plugin_id),
                );

                // Note: Actions are collected async via thread, so we can't process them
                // synchronously here. For detail_view, toasts etc. will fire after thread completes.
                // A future improvement could use channels for immediate processing.
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
