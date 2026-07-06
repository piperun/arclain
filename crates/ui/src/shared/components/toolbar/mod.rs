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
pub use types::{
    PluginEventDispatcher, PluginToolbarRenderer, ToolbarActions, ToolbarConfig, ToolbarState,
};

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
    plugin_renderer: PluginToolbarRenderer<'_>,
    plugin_dispatcher: PluginEventDispatcher<'_>,
) -> ToolbarActions {
    let mut actions = ToolbarActions::default();

    // Collect dialog open/close signals for processing after render.
    // These are pure UI-state mutations (no plugin work) and stay
    // synchronous; plugin events themselves go through the async
    // dispatcher below.
    let dialog_signals: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));

    // Pre-fetch plugin elements. Uses try_lock so a worker thread
    // holding the instance for a long-running event (e.g. DLSite
    // fetch) doesn't freeze the UI. On contention this plugin's
    // toolbar buttons are skipped for the current frame and reappear
    // once the lock releases.
    let mut plugin_elements = HashMap::new();
    if let Some(manager_arc) = plugin_manager {
        let manager = manager_arc.lock();
        let plugins = manager.list_plugins();
        for plugin in plugins.iter().filter(|p| p.enabled) {
            let pid = plugin.id.clone();
            if let Some(Some(layout)) = manager.try_with_plugin_instance(&pid, |instance| {
                instance
                    .get_ui_layout(PluginExtensionPoint::PluginButton)
                    .ok()
            }) {
                if let Some(layout) = layout {
                    plugin_elements.insert(pid, layout.flatten());
                }
            }
        }
    }

    let show_labels = shared
        .map(|s| s.signals().ui_preferences.get().show_button_labels)
        .unwrap_or(false);

    let ctx = ButtonContext {
        theme,
        can_go_back,
        can_go_forward,
        can_go_up,
        archive_loaded,
        has_selection,
        plugin_elements,
        show_labels,
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
                        buttons::render_button(
                            ui,
                            item,
                            &ctx,
                            state,
                            &mut actions,
                            plugin_renderer,
                        );
                    }
                });
            });

            // Visual separator between groups
            ui.separator();
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
                                    buttons::render_button(
                                        ui,
                                        item,
                                        &ctx,
                                        state,
                                        &mut actions,
                                        plugin_renderer,
                                    );
                                }
                            }
                        }
                    });
                });
            });
        }
    });

    // Process plugin events collected from render_button. Dialog
    // control prefixes stay synchronous (UI-state only); plugin
    // events go through the async dispatcher so the WASM call doesn't
    // block the UI thread.
    let dialog_sink = dialog_signals.clone();
    for (plugin_id, event_id, value) in actions.plugin_events.drain(..) {
        if event_id.starts_with("__dialog_open:") {
            let dialog_id = event_id.trim_start_matches("__dialog_open:").to_string();
            dialog_sink.lock().push((plugin_id, dialog_id));
            continue;
        }
        if event_id == "__dialog_close" {
            dialog_sink.lock().push((plugin_id, "__close".to_string()));
            continue;
        }

        // Delegate dispatch to the injected callback (constructed in
        // core/, where reaching into features/plugins is allowed).
        (plugin_dispatcher)(plugin_id, event_id, value);
    }

    // Process dialog signals (synchronous — pure signal mutation).
    if let Some(shared) = shared {
        let dialog_signal = shared.signals().plugin_dialog_state.clone();
        let mut dialog_state = dialog_signal.get();

        let dialog_sigs = dialog_signals.lock();
        for (plugin_id, dialog_id) in dialog_sigs.iter() {
            if dialog_id == "__close" {
                dialog_state.close_dialog();
            } else {
                dialog_state.open_dialog(plugin_id, dialog_id);
            }
        }

        dialog_signal.set(dialog_state);
    }

    actions
}
