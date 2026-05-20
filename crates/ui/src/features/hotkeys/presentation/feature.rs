use crate::features::hotkeys::presentation::KeyboardMouseSettingsState;
use crate::shared::SharedState;

/// Top-level feature struct for the hotkeys feature, owning the
/// keyboard/mouse settings page state.
pub struct HotkeysFeature {
    pub keyboard_mouse_state: KeyboardMouseSettingsState,
}

impl HotkeysFeature {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            keyboard_mouse_state: KeyboardMouseSettingsState::new(),
        }
    }
}
