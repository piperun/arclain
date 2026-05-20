//! Hotkey management feature
//!
//! Provides keyboard and mouse button shortcut handling with configurable bindings.

pub mod application;
pub mod domain;
pub mod presentation;

pub use application::hotkey_manager::HotkeyManager;
pub use domain::types::*;
pub use presentation::HotkeysFeature;
