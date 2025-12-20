//! Arclain Widgets - Reusable UI widget primitives
//!
//! This crate provides atomic UI widgets for the Arclain application:
//! - `TextButton` - Text button with semantic variants
//! - `IconButton` - Icon-only button
//! - `ToggleButton` - Button with selected/unselected state
//! - `ToggleSwitch` - On/off switch
//! - `SegmentedControl` - Two-option text selector
//! - `ThemedSlider` - Themed slider with value label
//! - `Chip` - Pill-shaped label/badge
//! - `CollapsibleSection` - Collapsible panel with theme support
//! - `Toast` / `Toaster` - Toast notification system

pub mod button;
pub mod collapsible_section;
pub mod icon_button;
pub mod segmented_control;
pub mod themed_slider;
pub mod toast;
pub mod toggle_button;
pub mod toggle_switch;

pub use button::{ButtonSize, TextButton};
pub use collapsible_section::CollapsibleSection;
pub use icon_button::{IconButton, IconButtonSize};
pub mod chips;
pub use chips::Chips;
pub use segmented_control::SegmentedControl;
pub use themed_slider::ThemedSlider;
pub use toast::{Toast, ToastLevel, Toaster};
pub use toggle_button::ToggleButton;
pub use toggle_switch::ToggleSwitch;
