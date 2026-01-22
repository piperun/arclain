//! Hotkey Manager
//!
//! Central service for hotkey detection and action dispatch.

use crate::features::hotkeys::domain::types::{
    HotkeyAction, HotkeyBinding, HotkeyBindings, InputKey, KeyboardKey, Modifiers, MouseButton,
};
use eframe::egui;
use std::collections::HashMap;

/// Central hotkey manager that checks for input and returns triggered actions
#[derive(Debug, Clone)]
pub struct HotkeyManager {
    bindings: HotkeyBindings,
}

impl Default for HotkeyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl HotkeyManager {
    /// Create a new HotkeyManager with default bindings
    pub fn new() -> Self {
        Self {
            bindings: HotkeyBindings::with_defaults(),
        }
    }

    /// Load bindings from database config
    pub fn from_config(config: &HashMap<String, String>) -> Self {
        Self {
            bindings: HotkeyBindings::from_config(config),
        }
    }

    /// Check current frame input and return any triggered actions
    pub fn check_input(&self, ctx: &egui::Context) -> Vec<HotkeyAction> {
        let mut triggered = Vec::new();

        ctx.input(|input| {
            // Get current modifiers state
            let modifiers = Modifiers {
                ctrl: input.modifiers.ctrl,
                alt: input.modifiers.alt,
                shift: input.modifiers.shift,
            };

            for event in &input.events {
                match event {
                    // Mouse Button Events
                    egui::Event::PointerButton {
                        button,
                        pressed: true,
                        ..
                    } => {
                        // DEBUG: Log all button presses to see what we're getting (DEBUG for visibility)
                        tracing::debug!(
                            "Raw PointerButton event: {:?} (Modifiers: {:?})",
                            button,
                            modifiers
                        );

                        let mouse_btn = match button {
                            egui::PointerButton::Extra1 => Some(MouseButton::Back),
                            egui::PointerButton::Extra2 => Some(MouseButton::Forward),
                            _ => None,
                        };

                        if let Some(mb) = mouse_btn {
                            tracing::debug!("Mapped to MouseButton: {:?}", mb);
                            if let Some(action) = self.bindings.find_action_for_input(
                                &InputKey::Mouse(mb),
                                &Modifiers::none(), // Mouse buttons ignore modifiers for now
                            ) {
                                tracing::debug!("Triggering action (Mouse): {:?}", action);
                                triggered.push(action);
                            } else {
                                tracing::debug!("No binding found for MouseButton: {:?}", mb);
                            }
                        }
                    }

                    // Keyboard Events
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: _, // We might want to allow repeat for some actions, sticking to refire for now
                        modifiers: _, // We use the global modifier state
                        physical_key: _,
                    } => {
                        if let Some(kb_key) = KeyboardKey::from_egui(*key) {
                            // Check for matching binding
                            if let Some(action) = self
                                .bindings
                                .find_action_for_input(&InputKey::Keyboard(kb_key), &modifiers)
                            {
                                // Smart focus handling:
                                // If a text widget is focused, only allow hotkeys that use modifiers (Ctrl/Alt)
                                // or are special keys (F-keys), avoiding interference with typing.
                                let is_text_input = !modifiers.ctrl && !modifiers.alt;
                                if input.focused && is_text_input {
                                    // Ignore simple key presses when typing
                                    continue;
                                }

                                tracing::debug!("Triggering action (Keyboard): {:?}", action);
                                triggered.push(action);
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

        triggered
    }

    /// Check if a specific action's hotkey is pressed this frame
    pub fn is_action_pressed(&self, ctx: &egui::Context, action: HotkeyAction) -> bool {
        self.check_input(ctx).contains(&action)
    }

    // --- Binding management ---

    /// Get binding for an action
    pub fn get_binding(&self, action: HotkeyAction) -> Option<&HotkeyBinding> {
        self.bindings.get(action)
    }

    /// Set binding for an action
    pub fn set_binding(&mut self, action: HotkeyAction, binding: HotkeyBinding) {
        self.bindings.set(action, binding);
    }

    /// Remove binding for an action
    pub fn remove_binding(&mut self, action: HotkeyAction) {
        self.bindings.remove(action);
    }

    /// Reset an action to default binding
    pub fn reset_to_default(&mut self, action: HotkeyAction) {
        self.bindings.reset_to_default(action);
    }

    /// Reset all bindings to defaults
    pub fn reset_all_to_defaults(&mut self) {
        self.bindings.reset_all_to_defaults();
    }

    /// Export bindings for database persistence
    pub fn to_config(&self) -> HashMap<String, String> {
        self.bindings.to_config()
    }

    /// Get all bindings
    pub fn all_bindings(&self) -> &HotkeyBindings {
        &self.bindings
    }

    /// Get display string for an action's current binding
    pub fn get_binding_display(&self, action: HotkeyAction) -> String {
        self.bindings
            .get(action)
            .map(|b| b.display_string())
            .unwrap_or_else(|| "Not set".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::hotkeys::domain::types::KeyboardKey;

    #[test]
    fn test_manager_has_defaults() {
        let manager = HotkeyManager::new();

        // All actions should have bindings
        for action in HotkeyAction::all() {
            assert!(manager.get_binding(action).is_some());
        }
    }

    #[test]
    fn test_manager_from_config() {
        let mut config = HashMap::new();
        config.insert(
            "navigate_back".to_string(),
            r#"{"key":{"Keyboard":"Backspace"},"modifiers":{"ctrl":false,"alt":false,"shift":false}}"#.to_string(),
        );

        let manager = HotkeyManager::from_config(&config);

        // Navigate back should be overridden
        let binding = manager.get_binding(HotkeyAction::NavigateBack).unwrap();
        assert!(matches!(
            binding.key,
            InputKey::Keyboard(KeyboardKey::Backspace)
        ));
    }

    #[test]
    fn test_manager_export_import() {
        let manager = HotkeyManager::new();
        let config = manager.to_config();

        let restored = HotkeyManager::from_config(&config);

        // Bindings should match
        for action in HotkeyAction::all() {
            assert_eq!(manager.get_binding(action), restored.get_binding(action),);
        }
    }
}
