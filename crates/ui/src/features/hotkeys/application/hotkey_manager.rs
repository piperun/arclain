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

/// Whether a *keyboard* hotkey should fire, given what egui is currently
/// doing with the keyboard. (Mouse-button hotkeys bypass this entirely.)
///
/// This is the central focus-respect rule that keeps app hotkeys from
/// fighting whatever owns the keyboard:
/// - A popup or context menu is open → it owns the keyboard; nothing else
///   fires.
/// - A widget holds keyboard focus (a text field is being edited) → only
///   global application shortcuts fire; anything contextual is suppressed
///   so it can't act on a keystroke meant for the editor. Keyed on the
///   action's intent ([`HotkeyAction::fires_while_typing`]), so it holds
///   for any chord the action is rebound to — not a hardcoded key list.
fn keyboard_hotkey_allowed(
    action: HotkeyAction,
    editor_has_keyboard: bool,
    popup_open: bool,
) -> bool {
    if popup_open {
        return false;
    }
    if editor_has_keyboard && !action.fires_while_typing() {
        return false;
    }
    true
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

        // Read what egui is doing with the keyboard before borrowing input,
        // so keyboard_hotkey_allowed can suppress hotkeys that would fight a
        // focused editor or an open popup/context menu. (These touch the
        // memory / pass-state locks; querying them outside the `input`
        // closure keeps the locking obvious.)
        let editor_has_keyboard = ctx.wants_keyboard_input();
        let popup_open = ctx.is_popup_open();

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
                                // Don't fire while a focused editor or open
                                // context menu owns the keyboard — see
                                // keyboard_hotkey_allowed.
                                if !keyboard_hotkey_allowed(action, editor_has_keyboard, popup_open)
                                {
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
    fn focused_editor_suppresses_contextual_hotkeys_only() {
        // Regression: Ctrl+A in the search box used to both select the query
        // text (egui) AND fire SelectAll ("select all files"). With a focused
        // editor, contextual actions are suppressed regardless of their chord,
        // while global app shortcuts still fire.
        let focused = true;
        let no_menu = false;
        assert!(
            !keyboard_hotkey_allowed(HotkeyAction::SelectAll, focused, no_menu),
            "SelectAll must not fire while a text field is focused"
        );
        assert!(
            !keyboard_hotkey_allowed(HotkeyAction::DeleteSelected, focused, no_menu),
            "DeleteSelected must not fire while typing"
        );
        assert!(
            !keyboard_hotkey_allowed(HotkeyAction::ExtractSelected, focused, no_menu),
            "ExtractSelected must not fire while typing"
        );
        assert!(
            keyboard_hotkey_allowed(HotkeyAction::OpenSettings, focused, no_menu),
            "OpenSettings is global — still fires while typing"
        );
        assert!(
            keyboard_hotkey_allowed(HotkeyAction::Search, focused, no_menu),
            "Search is global — still fires while typing"
        );
    }

    #[test]
    fn open_popup_suppresses_all_keyboard_hotkeys() {
        // An open popup / context menu owns the keyboard — not even global
        // shortcuts should fire underneath it.
        let popup_open = true;
        assert!(!keyboard_hotkey_allowed(
            HotkeyAction::SelectAll,
            false,
            popup_open
        ));
        assert!(!keyboard_hotkey_allowed(
            HotkeyAction::OpenSettings,
            false,
            popup_open
        ));
        assert!(!keyboard_hotkey_allowed(
            HotkeyAction::Search,
            true,
            popup_open
        ));
    }

    #[test]
    fn hotkeys_fire_normally_when_nothing_owns_the_keyboard() {
        // No focus, no menu: every action is allowed through.
        for action in HotkeyAction::all() {
            assert!(
                keyboard_hotkey_allowed(action, false, false),
                "{action:?} should fire when nothing owns the keyboard"
            );
        }
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
