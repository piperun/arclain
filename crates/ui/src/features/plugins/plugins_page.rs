//! Unified Plugin Page
//!
//! Drill-down view for managing plugins and their settings.

use crate::features::plugins::types::PluginsListState;
use crate::features::settings::types::SettingsAction;
use crate::shared::components::SettingsForm;
use crate::shared::theme::AppTheme;
use arclain_plugins::PluginManager;
use arclain_widgets::toggle_switch::ToggleSwitch;
use arclain_widgets::Chips;
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
    let action = None;
    let mut needs_refresh = false;

    if let Some(selected_id) = &state.selected_plugin {
        // --- Detail View ---
        if let Some(plugin_info) = state.plugins.iter().find(|p| &p.id == selected_id) {
            // We need to clone ID for the back closure to avoid borrow checker issues if we used state directly
            // But we are passing a closure that modifies state.
            // Wait, `on_back` takes generic `FnOnce`. We can't easily capture `state` if `render` owns it mutably?
            // `SettingsPage::show` takes `self`, so `on_back` closure is consumed.
            // BUT `render` has `state: &mut PluginsListState`.
            // If we construct `SettingsPage` with a closure that uses `state`, it borrows `state`.
            // Then `show` borrows `ui`. This should be fine.

            let mut back_clicked = false;

            crate::shared::components::SettingsHeader::new(&plugin_info.name)
                .description(format!(
                    "v{} by {}",
                    plugin_info.version,
                    plugin_info.author.as_deref().unwrap_or("Unknown")
                ))
                .on_back(|| {
                    back_clicked = true;
                })
                .show(ui, theme);

            SettingsForm::new().show(ui, theme, |ui| {
                if let Some(desc) = &plugin_info.description {
                    ui.label(egui::RichText::new(desc).color(theme.colors.on_surface_variant));
                    ui.add_space(16.0);
                }

                // Global Settings
                crate::shared::components::settings_form::SectionHeader::new("Global Settings")
                    .show(ui, &theme.colors);

                // Enabled/Disabled Toggle using SettingsRow
                let mut enabled = plugin_info.enabled;
                crate::shared::components::settings_form::SettingsRow::new("Plugin Status")
                    .description("Enable or disable this plugin completely.")
                    .action(|ui| {
                        if ui
                            .add(ToggleSwitch::new(&mut enabled).icons("⚡", "⏻"))
                            .changed()
                        {
                            // Handle toggle logic (same as before)
                            if let Some(mgr) = plugin_manager {
                                let res = if enabled {
                                    mgr.enable_plugin(&plugin_info.id)
                                } else {
                                    mgr.disable_plugin(&plugin_info.id)
                                };

                                if res.is_ok() {
                                    // Persist enabled state
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
                crate::shared::components::settings_form::SectionHeader::new(
                    "Plugin Configuration",
                )
                .show(ui, &theme.colors);

                if let Some(manager) = plugin_manager {
                    if plugin_info.loaded {
                        let mgr_arc =
                            if let Some(mgr_mutex) = app_state.lock().plugin_manager.clone() {
                                mgr_mutex
                            } else {
                                return;
                            };

                        let ui_result = if let Some(instance_arc) =
                            manager.get_plugin_instance(&plugin_info.id)
                        {
                            if let Some(mut instance) = instance_arc.try_lock() {
                                Some(instance.get_ui_layout(
                                    arclain_plugins::types::PluginExtensionPoint::MainPage,
                                ))
                            } else {
                                None // Busy
                            }
                        } else {
                            Some(Err(arclain_plugins::types::PluginError::NotFound(
                                plugin_info.id.clone(),
                            )))
                        };

                        match ui_result {
                            Some(Ok(ui_elements)) => {
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

                                    let mut event_callback =
                                        Box::new(move |id: &str, value: Option<String>| {
                                            // Clone data for the thread
                                            let mgr_thread = mgr_clone.clone();
                                            let pid_thread = plugin_id_clone.clone();
                                            let id_thread = id.to_string();
                                            let val_thread = value.clone();

                                            // Spawn thread to avoid blocking UI
                                            std::thread::spawn(move || {
                                                let mgr = mgr_thread.lock();
                                                // Blocking lock is fine in background thread
                                                if let Some(instance_arc) =
                                                    mgr.get_plugin_instance(&pid_thread)
                                                {
                                                    let mut instance = instance_arc.lock();
                                                    let _ = instance
                                                        .send_ui_event(&id_thread, val_thread);
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
                                    egui::RichText::new(format!("Error loading UI: {}", e))
                                        .color(theme.colors.error),
                                );
                            }
                            None => {
                                // Busy / Locked
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
                    } else {
                        ui.label(
                            egui::RichText::new("Plugin is not loaded.")
                                .color(theme.colors.on_surface_variant),
                        );
                    }
                }
            });

            if back_clicked {
                state.selected_plugin = None;
            }
        } else {
            // ID not found, reset
            state.selected_plugin = None;
        }
    } else {
        // --- List View ---

        let filter_query = state.filter_text.clone();

        // settings_page handles the header injection.
        SettingsForm::new().show(ui, theme, |ui| {
            ui.add_space(8.0);

            for plugin in &state.plugins {
                // Filter logic (Search is handled by us filtering here, text edit is in header)
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
                let response = egui::Frame::NONE
                    .fill(theme.colors.surface_variant.linear_multiply(0.3)) // Slight background
                    .inner_margin(12.0)
                    .corner_radius(6.0)
                    .stroke(egui::Stroke::new(
                        1.0,
                        theme.colors.outline.linear_multiply(0.2),
                    ))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // Status Icon
                            // Status Icon/Indicator
                            let (status_rect, _) = ui
                                .allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                            if ui.is_rect_visible(status_rect) {
                                let center = status_rect.center();

                                if !plugin.enabled {
                                    // DISABLED State (Power Icon, Gray)
                                    let color = theme.colors.outline;
                                    // Hollow circle
                                    ui.painter().circle_stroke(
                                        center,
                                        8.0,
                                        egui::Stroke::new(1.5, color),
                                    );
                                    // Icon
                                    ui.painter().text(
                                        center,
                                        egui::Align2::CENTER_CENTER,
                                        "⏻",
                                        egui::FontId::proportional(10.0),
                                        color,
                                    );
                                } else {
                                    // ENABLED State
                                    let status_color = plugin.status.color();

                                    match plugin.status {
                                        crate::features::plugins::types::PluginStatus::Ready => {
                                            // Actively Running/Ready (Green Glow + Lightning)
                                            // 1. Outer LED Glow
                                            ui.painter().circle_filled(
                                                center,
                                                10.0,
                                                status_color.linear_multiply(0.2),
                                            );
                                            // 2. Core Ring
                                            ui.painter().circle_stroke(
                                                center,
                                                8.0,
                                                egui::Stroke::new(2.0, status_color),
                                            );
                                            // 3. Icon
                                            ui.painter().text(
                                                center,
                                                egui::Align2::CENTER_CENTER,
                                                "⚡",
                                                egui::FontId::proportional(12.0),
                                                status_color,
                                            );
                                        }
                                        crate::features::plugins::types::PluginStatus::Loading => {
                                            // Loading (Blue + Spinner)
                                            ui.painter().circle_stroke(
                                                center,
                                                8.0,
                                                egui::Stroke::new(1.5, status_color),
                                            );
                                            ui.painter().text(
                                                center,
                                                egui::Align2::CENTER_CENTER,
                                                "⟳",
                                                egui::FontId::proportional(10.0),
                                                status_color,
                                            );
                                        }
                                        crate::features::plugins::types::PluginStatus::Error => {
                                            // Error (Red + Warning)
                                            ui.painter().circle_filled(
                                                center,
                                                8.0,
                                                status_color.linear_multiply(0.2),
                                            );
                                            ui.painter().circle_stroke(
                                                center,
                                                8.0,
                                                egui::Stroke::new(1.5, status_color),
                                            );
                                            ui.painter().text(
                                                center,
                                                egui::Align2::CENTER_CENTER,
                                                "⚠",
                                                egui::FontId::proportional(10.0),
                                                status_color,
                                            );
                                        }
                                        _ => {
                                            ui.painter().circle_stroke(
                                                center,
                                                6.0,
                                                egui::Stroke::new(1.0, status_color),
                                            );
                                        }
                                    }
                                }
                            }
                            ui.add_space(8.0);

                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(&plugin.name)
                                        .strong()
                                        .size(16.0)
                                        .color(theme.colors.on_surface),
                                );
                                ui.label(
                                    egui::RichText::new(
                                        plugin.description.as_deref().unwrap_or(""),
                                    )
                                    .color(theme.colors.on_surface_variant),
                                );

                                if state.show_permissions && !plugin.capabilities.is_empty() {
                                    ui.add_space(8.0);
                                    ui.horizontal_wrapped(|ui| {
                                        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                                        for cap in &plugin.capabilities {
                                            ui.add(arclain_widgets::Chips::new(cap));
                                        }
                                    });
                                }
                            });

                            // Right side info
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("v{}", plugin.version))
                                            .small()
                                            .color(theme.colors.on_surface_variant),
                                    );
                                },
                            );
                        });
                    })
                    .response;

                if response.interact(egui::Sense::click()).clicked() {
                    state.selected_plugin = Some(plugin.id.clone());
                }

                ui.add_space(4.0);
            }
        });
    }

    if needs_refresh {
        if let Some(manager) = plugin_manager {
            let state_lock = app_state.lock();
            state.update_from_manager(manager, &state_lock.user_config);
        }
    }

    action
}
