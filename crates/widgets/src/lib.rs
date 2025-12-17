//! Arclain Widgets - Reusable UI widget primitives
//!
//! This crate provides atomic UI widgets for the Arclain application:
//! - `TextButton` - Text button with semantic variants
//! - `IconButton` - Icon-only button
//! - `ToggleButton` - Button with selected/unselected state
//! - `ToggleSwitch` - On/off switch
//! - `Chip` - Pill-shaped label/badge
//! - `CollapsibleSection` - Collapsible panel with theme support

pub mod button;
pub mod collapsible_section;
pub mod icon_button;
pub mod toggle_button;
pub mod toggle_switch;

pub use button::{ButtonSize, TextButton};
pub use collapsible_section::CollapsibleSection;
pub use icon_button::{IconButton, IconButtonSize};
pub mod chips;
pub use chips::Chips;
pub use toggle_button::ToggleButton;
pub use toggle_switch::ToggleSwitch;
