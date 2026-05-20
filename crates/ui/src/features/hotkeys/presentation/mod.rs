//! Presentation layer for the hotkeys feature.

pub mod feature;
pub mod keyboard_mouse_page;

pub use feature::HotkeysFeature;
pub use keyboard_mouse_page::{KeyboardMouseSettingsState, render as render_keyboard_mouse};
