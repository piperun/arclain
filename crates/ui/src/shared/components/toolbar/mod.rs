use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_plugins::manager::PluginManager;
use arclain_plugins::types::PluginExtensionPoint;
use eframe::egui;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

mod buttons;
mod types;

use types::ButtonContext;
pub use types::{ToolbarActions, ToolbarConfig, ToolbarState};

pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut ToolbarState,
    can_go_back: bool,
    can_go_forward: bool,
    can_go_up: bool,
    archive_loaded: bool,
    has_selection: bool,
    _has_metadata: bool,
    config: Option<&ToolbarConfig>,
    plugin_manager: Option<&Arc<Mutex<PluginManager>>>,
    shared: Option<&SharedState>,
) -> ToolbarActions {
    let mut actions = ToolbarActions::default();

    // Collect plugin actions for processing after render
    let collected_actions: Arc<Mutex<Vec<(String, arclain_plugins::types::PluginAction)>>> =
        Arc::new(Mutex::new(Vec::new()));

    // Collect dialog signals for processing after render
    let dialog_signals: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));

    // Pre-fetch plugin elements
    let mut plugin_elements = HashMap::new();
    if let Some(manager_arc) = plugin_manager {
        let manager = manager_arc.lock();
        let plugins = manager.list_plugins();
        for plugin in plugins.iter().filter(|p| p.enabled) {
            let pid = plugin.id.clone();
            let _ = manager.with_plugin_instance(&pid, |instance| {
                if let Ok(layout) = instance.get_ui_layout(PluginExtensionPoint::PluginButton) {
                    plugin_elements.insert(pid.clone(), layout.flatten());
                }
                Ok::<_, anyhow::Error>(())
            });
        }
    }

    let ctx = ButtonContext {
        theme,
        shared,
        can_go_back,
        can_go_forward,
        can_go_up,
        archive_loaded,
        has_selection,
        plugin_elements,
    };

    // If no config, render nothing (or could have a fallback)
    let Some(config) = config else {
        return actions;
    };

    let groups = config.items_by_group();

    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(12.0, 0.0);

        // Track which groups we've rendered (for right-aligned panels)
        let mut rendered_panels = false;

        for (group_id, items) in &groups {
            // Panel toggles go to the right side
            if group_id.as_deref() == Some("panels") {
                rendered_panels = true;
                continue; // Render later
            }

            ui.scope(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
                ui.horizontal_centered(|ui| {
                    for item in items {
                        buttons::render_button(ui, item, &ctx, state, &mut actions);
                    }
                });
            });

            ui.add_space(4.0);
        }

        // Legacy plugin rendering removed (now handled via standard items)

        // Panel toggles - right aligned
        if rendered_panels {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
                    ui.horizontal_centered(|ui| {
                        // Find panels group and render in reverse (for right-to-left)
                        for (group_id, items) in groups.iter().rev() {
                            if group_id.as_deref() == Some("panels") {
                                for item in items.iter().rev() {
                                    buttons::render_button(ui, item, &ctx, state, &mut actions);
                                }
                            }
                        }
                    });
                });
            });
        }
    });

    // Process plugin events collected from render_button
    if let Some(manager_arc) = plugin_manager {
        let manager = manager_arc.lock();
        let actions_sink = collected_actions.clone();
        let dialog_sink = dialog_signals.clone();

        for (plugin_id, event_id, value) in actions.plugin_events.drain(..) {
            // Check for dialog control signals
            if event_id.starts_with("__dialog_open:") {
                let dialog_id = event_id.trim_start_matches("__dialog_open:").to_string();
                dialog_sink.lock().push((plugin_id.clone(), dialog_id));
                continue;
            }
            if event_id == "__dialog_close" {
                dialog_sink
                    .lock()
                    .push((plugin_id.clone(), "__close".to_string()));
                continue;
            }

            // Send to plugin
            let _ = manager.with_plugin_instance(&plugin_id, |instance| {
                if let Ok(returned_actions) = instance.send_ui_event(&event_id, value.clone()) {
                    let mut sink = actions_sink.lock();
                    for a in returned_actions {
                        sink.push((plugin_id.clone(), a));
                    }
                }
                Ok::<_, anyhow::Error>(())
            });
        }
    }

    // Process collected plugin actions and dialog signals
    if let Some(shared) = shared {
        let actions_list = collected_actions.lock();
        let mut toaster = shared.toaster.lock();
        let mut dialog_state = shared.plugin_dialog_state.lock();

        for (plugin_id, plugin_action) in actions_list.iter() {
            crate::features::plugins::actions::process_plugin_actions(
                vec![plugin_action.clone()],
                plugin_id,
                &mut dialog_state,
                &mut toaster,
                Some(&shared.refresh_requests),
            );
        }

        // Process dialog signals
        let dialog_sigs = dialog_signals.lock();
        for (plugin_id, dialog_id) in dialog_sigs.iter() {
            if dialog_id == "__close" {
                dialog_state.close_dialog();
            } else {
                dialog_state.open_dialog(plugin_id, dialog_id);
            }
        }
    }

    actions
}
