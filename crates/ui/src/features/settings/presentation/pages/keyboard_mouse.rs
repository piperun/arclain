//! Keyboard & Mouse Settings Page
//!
//! Settings page for configuring keyboard shortcuts and mouse button bindings.

use arclain_widgets::{ButtonSize, IconButton, IconButtonSize, TextButton};
use crate::features::hotkeys::{HotkeyAction, HotkeyManager};
use crate::features::settings::types::SettingsAction;
use crate::shared::components::settings_form::{SettingsForm, SettingsGroup};
use crate::shared::theme::AppTheme;
use arclain_theme::ThemeColors;
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
    SettingsForm::new().show(ui, theme, |ui| {
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
            render_category(ui, &theme.colors, state, category);
        }

        ui.add_space(8.0);

        // Reset all button using TextButton with icon
        if ui
            .add(
                TextButton::new(
                    format!(
                        "{} Reset All to Defaults",
                        egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE
                    ),
                    ButtonSize::Medium,
                )
                .with_theme_colors(&theme.colors),
            )
            .clicked()
        {
            state.manager.reset_all_to_defaults();
            state.dirty = true;
        }
    });

    None
}

fn render_category(
    ui: &mut egui::Ui,
    colors: &ThemeColors,
    state: &mut KeyboardMouseSettingsState,
    category: &str,
) {
    SettingsGroup::new(category)
        .content(|ui, colors| {
            // Filter actions by category
            let actions: Vec<HotkeyAction> = HotkeyAction::all()
                .into_iter()
                .filter(|a| a.category() == category)
                .collect();

            for action in actions {
                render_action_row(ui, colors, state, action);
            }
        })
        .show(ui, colors);
}

fn render_action_row(
    ui: &mut egui::Ui,
    colors: &ThemeColors,
    state: &mut KeyboardMouseSettingsState,
    action: HotkeyAction,
) {
    ui.horizontal(|ui| {
        // Action name
        ui.label(
            egui::RichText::new(action.display_name())
                .size(13.0)
                .color(colors.on_surface),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Reset button using IconButton
            if ui
                .add(
                    IconButton::new(egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE)
                        .size(IconButtonSize::Small)
                        .with_theme_colors(colors),
                )
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
                    .color(colors.primary)
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
