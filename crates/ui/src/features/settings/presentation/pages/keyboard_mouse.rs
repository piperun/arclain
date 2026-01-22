//! Keyboard & Mouse Settings Page
//!
//! Settings page for configuring keyboard shortcuts and mouse button bindings.

use crate::features::hotkeys::{HotkeyAction, HotkeyManager};
use crate::features::settings::types::SettingsAction;
use crate::shared::theme::AppTheme;
use eframe::egui;

/// State for the Keyboard & Mouse settings page
#[derive(Debug, Default)]
pub struct KeyboardMouseSettingsState {
    /// The hotkey manager with current bindings
    pub manager: HotkeyManager,
    /// Whether there are unsaved changes
    pub dirty: bool,
    /// Whether settings have been loaded from database
    pub loaded: bool,
}

impl KeyboardMouseSettingsState {
    pub fn new() -> Self {
        Self {
            manager: HotkeyManager::new(),
            dirty: false,
            loaded: false,
        }
    }

    /// Load settings from user config
    pub fn load_from_config(&mut self, bindings: std::collections::HashMap<String, String>) {
        self.manager = HotkeyManager::from_config(&bindings);
        self.loaded = true;
        self.dirty = false;
    }

    /// Export settings for persistence
    pub fn to_config(&self) -> std::collections::HashMap<String, String> {
        self.manager.to_config()
    }
}

/// Render the Keyboard & Mouse settings page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut KeyboardMouseSettingsState,
) -> Option<SettingsAction> {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

        // Description
        ui.label(
            egui::RichText::new(
                "Configure keyboard shortcuts and mouse button bindings for common actions.",
            )
            .size(13.0)
            .color(theme.colors.on_surface_variant),
        );

        ui.add_space(8.0);

        // Group actions by category
        let categories = [
            "Navigation",
            "Archive Operations",
            "Selection",
            "Application",
        ];

        for category in categories {
            render_category(ui, theme, state, category);
            ui.add_space(8.0);
        }

        ui.add_space(8.0);

        // Reset all button
        ui.horizontal(|ui| {
            if ui
                .button(format!(
                    "{} Reset All to Defaults",
                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE
                ))
                .clicked()
            {
                state.manager.reset_all_to_defaults();
                state.dirty = true;
            }
        });
    });

    None
}

fn render_category(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut KeyboardMouseSettingsState,
    category: &str,
) {
    egui::Frame::NONE
        .fill(theme.colors.surface_variant)
        .stroke(egui::Stroke::new(1.0, theme.colors.outline))
        .corner_radius(8.0)
        .inner_margin(16.0)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                // Category header
                ui.label(
                    egui::RichText::new(category)
                        .size(14.0)
                        .strong()
                        .color(theme.colors.on_surface),
                );
                ui.add_space(8.0);

                // Filter actions by category
                let actions: Vec<HotkeyAction> = HotkeyAction::all()
                    .into_iter()
                    .filter(|a| a.category() == category)
                    .collect();

                for action in actions {
                    render_action_row(ui, theme, state, action);
                }
            });
        });
}

fn render_action_row(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut KeyboardMouseSettingsState,
    action: HotkeyAction,
) {
    ui.horizontal(|ui| {
        // Action name
        ui.label(
            egui::RichText::new(action.display_name())
                .size(13.0)
                .color(theme.colors.on_surface),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Reset button
            if ui
                .add(egui::Button::new(egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE).small())
                .on_hover_text("Reset to default")
                .clicked()
            {
                state.manager.reset_to_default(action);
                state.dirty = true;
            }

            // Current binding display
            let binding_text = state.manager.get_binding_display(action);
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(&binding_text)
                    .size(12.0)
                    .color(theme.colors.primary)
                    .monospace(),
            );
        });
    });

    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_creation() {
        let state = KeyboardMouseSettingsState::new();
        assert!(!state.loaded);
        assert!(!state.dirty);
    }
}
