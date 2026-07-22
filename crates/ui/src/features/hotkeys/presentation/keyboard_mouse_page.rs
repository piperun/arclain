//! Keyboard & Mouse Settings Page
//!
//! Settings page for configuring keyboard shortcuts and mouse button bindings.

use crate::features::hotkeys::{
    HotkeyAction, HotkeyBinding, HotkeyManager, InputKey, KeyboardKey, Modifiers, MouseButton,
};
use crate::features::settings::types::SettingsAction;
use crate::shared::components::settings_form::{Form, SettingsGroup};
use crate::shared::theme::AppTheme;
use arclain_theme::ThemeColors;
use arclain_widgets::{ButtonSize, IconButton, IconButtonSize, TextButton};
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
    /// The action currently being edited (if any)
    pub editing_action: Option<HotkeyAction>,
    /// Error message for binding conflicts
    pub conflict_error: Option<String>,
}

impl KeyboardMouseSettingsState {
    pub fn new() -> Self {
        Self {
            manager: HotkeyManager::new(),
            dirty: false,
            loaded: false,
            editing_action: None,
            conflict_error: None,
        }
    }

    /// Load settings from user config
    pub fn load_from_config(&mut self, bindings: std::collections::HashMap<String, String>) {
        self.manager = HotkeyManager::from_config(&bindings);
        self.loaded = true;
        self.dirty = false;
        self.editing_action = None;
        self.conflict_error = None;
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
    Form::new().show(ui, theme, |ui| {
        // Description
        ui.label(
            egui::RichText::new(
                "Configure keyboard shortcuts and mouse button bindings for common actions.",
            )
            .size(13.0)
            .color(theme.colors.on_surface_variant),
        );

        ui.add_space(8.0);

        // Show conflict error if present
        if let Some(error) = &state.conflict_error {
            ui.colored_label(egui::Color32::RED, error);
            ui.add_space(8.0);
        }

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
    let is_editing = state.editing_action == Some(action);

    ui.horizontal(|ui| {
        // Action name
        ui.label(
            egui::RichText::new(action.display_name())
                .size(13.0)
                .color(colors.on_surface),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if is_editing {
                // Cancel button when editing
                if ui
                    .add(
                        IconButton::new(egui_phosphor::regular::X)
                            .size(IconButtonSize::Small)
                            .with_theme_colors(colors),
                    )
                    .on_hover_text("Cancel editing")
                    .clicked()
                {
                    state.editing_action = None;
                    state.conflict_error = None;
                }

                ui.add_space(8.0);

                // Show "Press a key..." message
                ui.label(
                    egui::RichText::new("Press any key or mouse button...")
                        .size(12.0)
                        .color(colors.on_surface_variant)
                        .italics(),
                );

                // Capture input
                ui.ctx().input(|input| {
                    // Get current modifiers
                    let modifiers = Modifiers {
                        ctrl: input.modifiers.ctrl,
                        alt: input.modifiers.alt,
                        shift: input.modifiers.shift,
                    };

                    // Check for mouse button events
                    for event in &input.events {
                        match event {
                            egui::Event::PointerButton {
                                button,
                                pressed: true,
                                ..
                            } => {
                                let mouse_btn = match button {
                                    egui::PointerButton::Extra1 => Some(MouseButton::Back),
                                    egui::PointerButton::Extra2 => Some(MouseButton::Forward),
                                    _ => None,
                                };

                                if let Some(mb) = mouse_btn {
                                    let new_binding =
                                        HotkeyBinding::new(InputKey::Mouse(mb), Modifiers::none());
                                    try_set_binding(state, action, new_binding);
                                }
                            }

                            egui::Event::Key {
                                key,
                                pressed: true,
                                repeat: false,
                                ..
                            } => {
                                if let Some(kb_key) = KeyboardKey::from_egui(*key) {
                                    let new_binding = HotkeyBinding::new(
                                        InputKey::Keyboard(kb_key),
                                        modifiers.clone(),
                                    );
                                    try_set_binding(state, action, new_binding);
                                }
                            }
                            _ => {}
                        }
                    }
                });
            } else {
                // Normal mode: Edit and Reset buttons
                // Reset button
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
                    state.conflict_error = None;
                }

                ui.add_space(4.0);

                // Edit button
                if ui
                    .add(
                        IconButton::new(egui_phosphor::regular::PENCIL)
                            .size(IconButtonSize::Small)
                            .with_theme_colors(colors),
                    )
                    .on_hover_text("Change binding")
                    .clicked()
                {
                    state.editing_action = Some(action);
                    state.conflict_error = None;
                }

                ui.add_space(8.0);

                // Current binding display
                let binding_text = state.manager.get_binding_display(action);
                ui.label(
                    egui::RichText::new(&binding_text)
                        .size(12.0)
                        .color(colors.primary)
                        .monospace(),
                );
            }
        });
    });

    ui.add_space(4.0);
}

/// Try to set a new binding, checking for conflicts
fn try_set_binding(
    state: &mut KeyboardMouseSettingsState,
    action: HotkeyAction,
    new_binding: HotkeyBinding,
) {
    // Check for conflicts (same binding used by another action)
    let all_bindings = state.manager.all_bindings().all();
    for (other_action, other_binding) in all_bindings {
        if *other_action != action && *other_binding == new_binding {
            state.conflict_error = Some(format!(
                "Binding {} is already used by '{}'",
                new_binding.display_string(),
                other_action.display_name()
            ));
            state.editing_action = None;
            return;
        }
    }

    // No conflict, set the binding
    state.manager.set_binding(action, new_binding);
    state.dirty = true;
    state.editing_action = None;
    state.conflict_error = None;
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
