//! Hotkey domain types
//!
//! Defines actions, bindings, and input types for the hotkey system.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Extended mouse button enum for back/forward buttons
/// egui's PointerButton only has Primary/Secondary/Middle, but Extra1/Extra2 exist
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    /// Mouse back button (Mouse4 / Extra1)
    Back,
    /// Mouse forward button (Mouse5 / Extra2)
    Forward,
}

impl MouseButton {
    pub fn display_name(&self) -> &'static str {
        match self {
            MouseButton::Back => "Mouse Back (Mouse4)",
            MouseButton::Forward => "Mouse Forward (Mouse5)",
        }
    }
}

/// Modifier keys that can be held with a hotkey
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Modifiers {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn ctrl() -> Self {
        Self {
            ctrl: true,
            ..Default::default()
        }
    }

    pub fn alt() -> Self {
        Self {
            alt: true,
            ..Default::default()
        }
    }

    pub fn ctrl_shift() -> Self {
        Self {
            ctrl: true,
            shift: true,
            ..Default::default()
        }
    }

    pub fn alt_shift() -> Self {
        Self {
            alt: true,
            shift: true,
            ..Default::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.ctrl && !self.alt && !self.shift
    }

    pub fn display_string(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        parts.join("+")
    }
}

/// Represents a key or mouse button that can be bound
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InputKey {
    /// A keyboard key
    Keyboard(KeyboardKey),
    /// A mouse button (back/forward)
    Mouse(MouseButton),
}

/// Keyboard keys that can be bound to hotkeys
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyboardKey {
    // Letters
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    // Numbers
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    // Function keys
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    // Navigation
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    // Special
    Backspace,
    Delete,
    Enter,
    Escape,
    Tab,
    Space,
    Comma,
}

impl KeyboardKey {
    pub fn display_name(&self) -> &'static str {
        match self {
            KeyboardKey::A => "A",
            KeyboardKey::B => "B",
            KeyboardKey::C => "C",
            KeyboardKey::D => "D",
            KeyboardKey::E => "E",
            KeyboardKey::F => "F",
            KeyboardKey::G => "G",
            KeyboardKey::H => "H",
            KeyboardKey::I => "I",
            KeyboardKey::J => "J",
            KeyboardKey::K => "K",
            KeyboardKey::L => "L",
            KeyboardKey::M => "M",
            KeyboardKey::N => "N",
            KeyboardKey::O => "O",
            KeyboardKey::P => "P",
            KeyboardKey::Q => "Q",
            KeyboardKey::R => "R",
            KeyboardKey::S => "S",
            KeyboardKey::T => "T",
            KeyboardKey::U => "U",
            KeyboardKey::V => "V",
            KeyboardKey::W => "W",
            KeyboardKey::X => "X",
            KeyboardKey::Y => "Y",
            KeyboardKey::Z => "Z",
            KeyboardKey::Num0 => "0",
            KeyboardKey::Num1 => "1",
            KeyboardKey::Num2 => "2",
            KeyboardKey::Num3 => "3",
            KeyboardKey::Num4 => "4",
            KeyboardKey::Num5 => "5",
            KeyboardKey::Num6 => "6",
            KeyboardKey::Num7 => "7",
            KeyboardKey::Num8 => "8",
            KeyboardKey::Num9 => "9",
            KeyboardKey::F1 => "F1",
            KeyboardKey::F2 => "F2",
            KeyboardKey::F3 => "F3",
            KeyboardKey::F4 => "F4",
            KeyboardKey::F5 => "F5",
            KeyboardKey::F6 => "F6",
            KeyboardKey::F7 => "F7",
            KeyboardKey::F8 => "F8",
            KeyboardKey::F9 => "F9",
            KeyboardKey::F10 => "F10",
            KeyboardKey::F11 => "F11",
            KeyboardKey::F12 => "F12",
            KeyboardKey::Left => "←",
            KeyboardKey::Right => "→",
            KeyboardKey::Up => "↑",
            KeyboardKey::Down => "↓",
            KeyboardKey::Home => "Home",
            KeyboardKey::End => "End",
            KeyboardKey::PageUp => "Page Up",
            KeyboardKey::PageDown => "Page Down",
            KeyboardKey::Backspace => "Backspace",
            KeyboardKey::Delete => "Delete",
            KeyboardKey::Enter => "Enter",
            KeyboardKey::Escape => "Esc",
            KeyboardKey::Tab => "Tab",
            KeyboardKey::Space => "Space",
            KeyboardKey::Comma => ",",
        }
    }

    /// Convert from egui Key
    pub fn from_egui(key: egui::Key) -> Option<Self> {
        Some(match key {
            egui::Key::A => KeyboardKey::A,
            egui::Key::B => KeyboardKey::B,
            egui::Key::C => KeyboardKey::C,
            egui::Key::D => KeyboardKey::D,
            egui::Key::E => KeyboardKey::E,
            egui::Key::F => KeyboardKey::F,
            egui::Key::G => KeyboardKey::G,
            egui::Key::H => KeyboardKey::H,
            egui::Key::I => KeyboardKey::I,
            egui::Key::J => KeyboardKey::J,
            egui::Key::K => KeyboardKey::K,
            egui::Key::L => KeyboardKey::L,
            egui::Key::M => KeyboardKey::M,
            egui::Key::N => KeyboardKey::N,
            egui::Key::O => KeyboardKey::O,
            egui::Key::P => KeyboardKey::P,
            egui::Key::Q => KeyboardKey::Q,
            egui::Key::R => KeyboardKey::R,
            egui::Key::S => KeyboardKey::S,
            egui::Key::T => KeyboardKey::T,
            egui::Key::U => KeyboardKey::U,
            egui::Key::V => KeyboardKey::V,
            egui::Key::W => KeyboardKey::W,
            egui::Key::X => KeyboardKey::X,
            egui::Key::Y => KeyboardKey::Y,
            egui::Key::Z => KeyboardKey::Z,
            egui::Key::Num0 => KeyboardKey::Num0,
            egui::Key::Num1 => KeyboardKey::Num1,
            egui::Key::Num2 => KeyboardKey::Num2,
            egui::Key::Num3 => KeyboardKey::Num3,
            egui::Key::Num4 => KeyboardKey::Num4,
            egui::Key::Num5 => KeyboardKey::Num5,
            egui::Key::Num6 => KeyboardKey::Num6,
            egui::Key::Num7 => KeyboardKey::Num7,
            egui::Key::Num8 => KeyboardKey::Num8,
            egui::Key::Num9 => KeyboardKey::Num9,
            egui::Key::F1 => KeyboardKey::F1,
            egui::Key::F2 => KeyboardKey::F2,
            egui::Key::F3 => KeyboardKey::F3,
            egui::Key::F4 => KeyboardKey::F4,
            egui::Key::F5 => KeyboardKey::F5,
            egui::Key::F6 => KeyboardKey::F6,
            egui::Key::F7 => KeyboardKey::F7,
            egui::Key::F8 => KeyboardKey::F8,
            egui::Key::F9 => KeyboardKey::F9,
            egui::Key::F10 => KeyboardKey::F10,
            egui::Key::F11 => KeyboardKey::F11,
            egui::Key::F12 => KeyboardKey::F12,
            egui::Key::ArrowLeft => KeyboardKey::Left,
            egui::Key::ArrowRight => KeyboardKey::Right,
            egui::Key::ArrowUp => KeyboardKey::Up,
            egui::Key::ArrowDown => KeyboardKey::Down,
            egui::Key::Home => KeyboardKey::Home,
            egui::Key::End => KeyboardKey::End,
            egui::Key::PageUp => KeyboardKey::PageUp,
            egui::Key::PageDown => KeyboardKey::PageDown,
            egui::Key::Backspace => KeyboardKey::Backspace,
            egui::Key::Delete => KeyboardKey::Delete,
            egui::Key::Enter => KeyboardKey::Enter,
            egui::Key::Escape => KeyboardKey::Escape,
            egui::Key::Tab => KeyboardKey::Tab,
            egui::Key::Space => KeyboardKey::Space,
            _ => return None,
        })
    }

    /// Convert to egui Key
    pub fn to_egui(&self) -> egui::Key {
        match self {
            KeyboardKey::A => egui::Key::A,
            KeyboardKey::B => egui::Key::B,
            KeyboardKey::C => egui::Key::C,
            KeyboardKey::D => egui::Key::D,
            KeyboardKey::E => egui::Key::E,
            KeyboardKey::F => egui::Key::F,
            KeyboardKey::G => egui::Key::G,
            KeyboardKey::H => egui::Key::H,
            KeyboardKey::I => egui::Key::I,
            KeyboardKey::J => egui::Key::J,
            KeyboardKey::K => egui::Key::K,
            KeyboardKey::L => egui::Key::L,
            KeyboardKey::M => egui::Key::M,
            KeyboardKey::N => egui::Key::N,
            KeyboardKey::O => egui::Key::O,
            KeyboardKey::P => egui::Key::P,
            KeyboardKey::Q => egui::Key::Q,
            KeyboardKey::R => egui::Key::R,
            KeyboardKey::S => egui::Key::S,
            KeyboardKey::T => egui::Key::T,
            KeyboardKey::U => egui::Key::U,
            KeyboardKey::V => egui::Key::V,
            KeyboardKey::W => egui::Key::W,
            KeyboardKey::X => egui::Key::X,
            KeyboardKey::Y => egui::Key::Y,
            KeyboardKey::Z => egui::Key::Z,
            KeyboardKey::Num0 => egui::Key::Num0,
            KeyboardKey::Num1 => egui::Key::Num1,
            KeyboardKey::Num2 => egui::Key::Num2,
            KeyboardKey::Num3 => egui::Key::Num3,
            KeyboardKey::Num4 => egui::Key::Num4,
            KeyboardKey::Num5 => egui::Key::Num5,
            KeyboardKey::Num6 => egui::Key::Num6,
            KeyboardKey::Num7 => egui::Key::Num7,
            KeyboardKey::Num8 => egui::Key::Num8,
            KeyboardKey::Num9 => egui::Key::Num9,
            KeyboardKey::F1 => egui::Key::F1,
            KeyboardKey::F2 => egui::Key::F2,
            KeyboardKey::F3 => egui::Key::F3,
            KeyboardKey::F4 => egui::Key::F4,
            KeyboardKey::F5 => egui::Key::F5,
            KeyboardKey::F6 => egui::Key::F6,
            KeyboardKey::F7 => egui::Key::F7,
            KeyboardKey::F8 => egui::Key::F8,
            KeyboardKey::F9 => egui::Key::F9,
            KeyboardKey::F10 => egui::Key::F10,
            KeyboardKey::F11 => egui::Key::F11,
            KeyboardKey::F12 => egui::Key::F12,
            KeyboardKey::Left => egui::Key::ArrowLeft,
            KeyboardKey::Right => egui::Key::ArrowRight,
            KeyboardKey::Up => egui::Key::ArrowUp,
            KeyboardKey::Down => egui::Key::ArrowDown,
            KeyboardKey::Home => egui::Key::Home,
            KeyboardKey::End => egui::Key::End,
            KeyboardKey::PageUp => egui::Key::PageUp,
            KeyboardKey::PageDown => egui::Key::PageDown,
            KeyboardKey::Backspace => egui::Key::Backspace,
            KeyboardKey::Delete => egui::Key::Delete,
            KeyboardKey::Enter => egui::Key::Enter,
            KeyboardKey::Escape => egui::Key::Escape,
            KeyboardKey::Tab => egui::Key::Tab,
            KeyboardKey::Space => egui::Key::Space,
            KeyboardKey::Comma => egui::Key::Comma,
        }
    }
}

/// A complete hotkey binding (key + modifiers)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HotkeyBinding {
    pub key: InputKey,
    pub modifiers: Modifiers,
}

impl HotkeyBinding {
    pub fn new(key: InputKey, modifiers: Modifiers) -> Self {
        Self { key, modifiers }
    }

    pub fn keyboard(key: KeyboardKey, modifiers: Modifiers) -> Self {
        Self::new(InputKey::Keyboard(key), modifiers)
    }

    pub fn mouse(button: MouseButton) -> Self {
        Self::new(InputKey::Mouse(button), Modifiers::none())
    }

    pub fn display_string(&self) -> String {
        let key_str = match &self.key {
            InputKey::Keyboard(k) => k.display_name().to_string(),
            InputKey::Mouse(m) => m.display_name().to_string(),
        };

        if self.modifiers.is_empty() {
            key_str
        } else {
            format!("{}+{}", self.modifiers.display_string(), key_str)
        }
    }

    /// Serialize to JSON string for database storage
    pub fn to_json(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }

    /// Deserialize from JSON string
    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}

/// Actions that can have hotkeys bound to them
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HotkeyAction {
    // Navigation
    NavigateBack,
    NavigateForward,
    NavigateUp,
    NavigateToRoot,

    // Archive operations
    OpenArchive,
    ExtractSelected,
    ExtractAll,
    DeleteSelected,

    // Selection
    SelectAll,

    // App
    OpenSettings,
    Search,
}

impl HotkeyAction {
    /// Get the unique identifier for this action
    pub fn id(&self) -> &'static str {
        match self {
            HotkeyAction::NavigateBack => "navigate_back",
            HotkeyAction::NavigateForward => "navigate_forward",
            HotkeyAction::NavigateUp => "navigate_up",
            HotkeyAction::NavigateToRoot => "navigate_to_root",
            HotkeyAction::OpenArchive => "open_archive",
            HotkeyAction::ExtractSelected => "extract_selected",
            HotkeyAction::ExtractAll => "extract_all",
            HotkeyAction::DeleteSelected => "delete_selected",
            HotkeyAction::SelectAll => "select_all",
            HotkeyAction::OpenSettings => "open_settings",
            HotkeyAction::Search => "search",
        }
    }

    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            HotkeyAction::NavigateBack => "Navigate Back",
            HotkeyAction::NavigateForward => "Navigate Forward",
            HotkeyAction::NavigateUp => "Navigate Up",
            HotkeyAction::NavigateToRoot => "Navigate to Root",
            HotkeyAction::OpenArchive => "Open Archive",
            HotkeyAction::ExtractSelected => "Extract Selected",
            HotkeyAction::ExtractAll => "Extract All",
            HotkeyAction::DeleteSelected => "Delete Selected",
            HotkeyAction::SelectAll => "Select All",
            HotkeyAction::OpenSettings => "Open Settings",
            HotkeyAction::Search => "Search",
        }
    }

    /// Get description for tooltips
    pub fn description(&self) -> &'static str {
        match self {
            HotkeyAction::NavigateBack => "Go back to the previous folder",
            HotkeyAction::NavigateForward => "Go forward in navigation history",
            HotkeyAction::NavigateUp => "Go up to the parent folder",
            HotkeyAction::NavigateToRoot => "Go to the archive root",
            HotkeyAction::OpenArchive => "Open an archive file",
            HotkeyAction::ExtractSelected => "Extract selected files",
            HotkeyAction::ExtractAll => "Extract all files from archive",
            HotkeyAction::DeleteSelected => "Delete selected files",
            HotkeyAction::SelectAll => "Select all files in current view",
            HotkeyAction::OpenSettings => "Open settings page",
            HotkeyAction::Search => "Focus the search box",
        }
    }

    /// Get the default binding for this action
    pub fn default_binding(&self) -> Option<HotkeyBinding> {
        Some(match self {
            HotkeyAction::NavigateBack => HotkeyBinding::mouse(MouseButton::Back),
            HotkeyAction::NavigateForward => HotkeyBinding::mouse(MouseButton::Forward),
            HotkeyAction::NavigateUp => HotkeyBinding::keyboard(KeyboardKey::Up, Modifiers::alt()),
            HotkeyAction::NavigateToRoot => {
                HotkeyBinding::keyboard(KeyboardKey::Home, Modifiers::alt())
            }
            HotkeyAction::OpenArchive => HotkeyBinding::keyboard(KeyboardKey::O, Modifiers::ctrl()),
            HotkeyAction::ExtractSelected => {
                HotkeyBinding::keyboard(KeyboardKey::E, Modifiers::ctrl())
            }
            HotkeyAction::ExtractAll => {
                HotkeyBinding::keyboard(KeyboardKey::E, Modifiers::ctrl_shift())
            }
            HotkeyAction::DeleteSelected => {
                HotkeyBinding::keyboard(KeyboardKey::Delete, Modifiers::none())
            }
            HotkeyAction::SelectAll => HotkeyBinding::keyboard(KeyboardKey::A, Modifiers::ctrl()),
            HotkeyAction::OpenSettings => {
                HotkeyBinding::keyboard(KeyboardKey::Comma, Modifiers::ctrl())
            }
            HotkeyAction::Search => HotkeyBinding::keyboard(KeyboardKey::F, Modifiers::ctrl()),
        })
    }

    /// Get action category for grouping in UI
    pub fn category(&self) -> &'static str {
        match self {
            HotkeyAction::NavigateBack
            | HotkeyAction::NavigateForward
            | HotkeyAction::NavigateUp
            | HotkeyAction::NavigateToRoot => "Navigation",

            HotkeyAction::OpenArchive
            | HotkeyAction::ExtractSelected
            | HotkeyAction::ExtractAll
            | HotkeyAction::DeleteSelected => "Archive Operations",

            HotkeyAction::SelectAll => "Selection",

            HotkeyAction::OpenSettings | HotkeyAction::Search => "Application",
        }
    }

    /// Get all available actions
    pub fn all() -> Vec<HotkeyAction> {
        vec![
            HotkeyAction::NavigateBack,
            HotkeyAction::NavigateForward,
            HotkeyAction::NavigateUp,
            HotkeyAction::NavigateToRoot,
            HotkeyAction::OpenArchive,
            HotkeyAction::ExtractSelected,
            HotkeyAction::ExtractAll,
            HotkeyAction::DeleteSelected,
            HotkeyAction::SelectAll,
            HotkeyAction::OpenSettings,
            HotkeyAction::Search,
        ]
    }

    /// Parse from action ID string
    pub fn from_id(id: &str) -> Option<HotkeyAction> {
        Some(match id {
            "navigate_back" => HotkeyAction::NavigateBack,
            "navigate_forward" => HotkeyAction::NavigateForward,
            "navigate_up" => HotkeyAction::NavigateUp,
            "navigate_to_root" => HotkeyAction::NavigateToRoot,
            "open_archive" => HotkeyAction::OpenArchive,
            "extract_selected" => HotkeyAction::ExtractSelected,
            "extract_all" => HotkeyAction::ExtractAll,
            "delete_selected" => HotkeyAction::DeleteSelected,
            "select_all" => HotkeyAction::SelectAll,
            "open_settings" => HotkeyAction::OpenSettings,
            "search" => HotkeyAction::Search,
            _ => return None,
        })
    }
}

/// Collection of all hotkey bindings
#[derive(Debug, Clone, Default)]
pub struct HotkeyBindings {
    bindings: HashMap<HotkeyAction, HotkeyBinding>,
}

impl HotkeyBindings {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with all default bindings
    pub fn with_defaults() -> Self {
        let mut bindings = HashMap::new();
        for action in HotkeyAction::all() {
            if let Some(binding) = action.default_binding() {
                bindings.insert(action, binding);
            }
        }
        Self { bindings }
    }

    /// Load from database config
    pub fn from_config(config: &HashMap<String, String>) -> Self {
        let mut bindings = Self::with_defaults();

        for (action_id, binding_json) in config {
            if let Some(action) = HotkeyAction::from_id(action_id) {
                if let Some(binding) = HotkeyBinding::from_json(binding_json) {
                    bindings.bindings.insert(action, binding);
                }
            }
        }

        bindings
    }

    /// Export to database config format
    pub fn to_config(&self) -> HashMap<String, String> {
        let mut config = HashMap::new();
        for (action, binding) in &self.bindings {
            if let Some(json) = binding.to_json() {
                config.insert(action.id().to_string(), json);
            }
        }
        config
    }

    /// Get binding for an action
    pub fn get(&self, action: HotkeyAction) -> Option<&HotkeyBinding> {
        self.bindings.get(&action)
    }

    /// Set binding for an action
    pub fn set(&mut self, action: HotkeyAction, binding: HotkeyBinding) {
        self.bindings.insert(action, binding);
    }

    /// Remove binding for an action
    pub fn remove(&mut self, action: HotkeyAction) {
        self.bindings.remove(&action);
    }

    /// Reset an action to its default binding
    pub fn reset_to_default(&mut self, action: HotkeyAction) {
        if let Some(binding) = action.default_binding() {
            self.bindings.insert(action, binding);
        } else {
            self.bindings.remove(&action);
        }
    }

    /// Reset all bindings to defaults
    pub fn reset_all_to_defaults(&mut self) {
        *self = Self::with_defaults();
    }

    /// Find action bound to a specific input
    pub fn find_action_for_input(
        &self,
        key: &InputKey,
        modifiers: &Modifiers,
    ) -> Option<HotkeyAction> {
        for (action, binding) in &self.bindings {
            if binding.key == *key && binding.modifiers == *modifiers {
                return Some(*action);
            }
        }
        None
    }

    /// Get all bindings
    pub fn all(&self) -> &HashMap<HotkeyAction, HotkeyBinding> {
        &self.bindings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_actions_have_defaults() {
        for action in HotkeyAction::all() {
            assert!(
                action.default_binding().is_some(),
                "Action {:?} has no default binding",
                action
            );
        }
    }

    #[test]
    fn test_binding_serialization_roundtrip() {
        let binding = HotkeyBinding::keyboard(KeyboardKey::O, Modifiers::ctrl());
        let json = binding.to_json().unwrap();
        let restored = HotkeyBinding::from_json(&json).unwrap();
        assert_eq!(binding, restored);
    }

    #[test]
    fn test_mouse_binding_serialization() {
        let binding = HotkeyBinding::mouse(MouseButton::Back);
        let json = binding.to_json().unwrap();
        let restored = HotkeyBinding::from_json(&json).unwrap();
        assert_eq!(binding, restored);
    }

    #[test]
    fn test_display_strings() {
        let binding = HotkeyBinding::keyboard(KeyboardKey::O, Modifiers::ctrl());
        assert_eq!(binding.display_string(), "Ctrl+O");

        let mouse_binding = HotkeyBinding::mouse(MouseButton::Back);
        assert!(mouse_binding.display_string().contains("Mouse"));
    }

    #[test]
    fn test_bindings_collection() {
        let bindings = HotkeyBindings::with_defaults();

        // Verify all actions have bindings
        for action in HotkeyAction::all() {
            assert!(bindings.get(action).is_some());
        }

        // Test find action for input
        let action =
            bindings.find_action_for_input(&InputKey::Mouse(MouseButton::Back), &Modifiers::none());
        assert_eq!(action, Some(HotkeyAction::NavigateBack));
    }
}
