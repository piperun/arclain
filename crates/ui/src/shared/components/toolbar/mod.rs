//! The archive toolbar: stored [`ToolbarConfig`] items, drawn in their
//! configured groups and order.
//!
//! Deliberately plugin-agnostic. An item whose action is a plugin is
//! drawn by an injected [`PluginToolbarRenderer`] the host supplies —
//! nothing here knows what a plugin, a plugin UI document, or a plugin
//! session is, and nothing here has to be told when one changes.

use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use eframe::egui;

mod buttons;
mod types;

use types::ButtonContext;
pub use types::{PluginToolbarRenderer, ToolbarActions, ToolbarConfig, ToolbarState};

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
    shared: Option<&SharedState>,
    plugin_renderer: PluginToolbarRenderer<'_>,
) -> ToolbarActions {
    let mut actions = ToolbarActions::default();

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

    actions
}
