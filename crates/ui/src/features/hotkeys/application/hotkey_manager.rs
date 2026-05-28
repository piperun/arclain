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

/// Should a focused text widget swallow this chord instead of letting it
/// fire an app hotkey?
///
/// True for bare keys (no Ctrl/Alt — that's typing) and for the standard
/// text-editing chords (Ctrl+A/C/V/X/Z/Y) that egui's `TextEdit` consumes
/// on its own. Without this, Ctrl+A in the search box would both select the
/// query text and trigger the archive's "select all files" hotkey.
fn swallowed_by_text_focus(key: egui::Key, modifiers: &Modifiers) -> bool {
    if !modifiers.ctrl && !modifiers.alt {
        return true; // bare key — it's typing, not a shortcut
    }
    if modifiers.ctrl && !modifiers.alt {
        // Shift stays allowed so Ctrl+Shift+Z (redo) is covered too.
        return matches!(
            key,
            egui::Key::A
                | egui::Key::C
                | egui::Key::V
                | egui::Key::X
                | egui::Key::Z
                | egui::Key::Y
        );
    }
    false
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
                                // When a text widget holds keyboard focus it owns
                                // both bare keys (typing) and the standard editing
                                // chords (Ctrl+A/C/V/X/Z/Y) that egui's TextEdit
                                // handles internally. Firing an app hotkey for those
                                // double-handles the key — e.g. Ctrl+A would both
                                // select the search text AND select every file in the
                                // archive. Let the focused editor swallow them.
                                if input.focused && swallowed_by_text_focus(*key, &modifiers) {
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
    fn ctrl_a_is_swallowed_by_a_focused_text_widget() {
        // Regression: Ctrl+A in the search box used to both select the query
        // text (egui) AND fire SelectAll (archive "select all files"). A
        // focused editor must swallow the editing chords so the app hotkey
        // doesn't double-handle them.
        let ctrl = Modifiers::ctrl();
        for key in [
            egui::Key::A,
            egui::Key::C,
            egui::Key::V,
            egui::Key::X,
            egui::Key::Z,
            egui::Key::Y,
        ] {
            assert!(
                swallowed_by_text_focus(key, &ctrl),
                "Ctrl+{key:?} should be swallowed by a focused text widget"
            );
        }
    }

    #[test]
    fn non_editing_chords_still_fire_while_typing() {
        // Ctrl+, (OpenSettings) isn't a text-editing chord, so a focused
        // search box must NOT swallow it.
        assert!(!swallowed_by_text_focus(egui::Key::Comma, &Modifiers::ctrl()));
    }

    #[test]
    fn bare_keys_are_swallowed_while_typing() {
        // Bare keys (no Ctrl/Alt) are typing — never steal them for hotkeys.
        assert!(swallowed_by_text_focus(egui::Key::Delete, &Modifiers::none()));
        assert!(swallowed_by_text_focus(egui::Key::A, &Modifiers::none()));
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
