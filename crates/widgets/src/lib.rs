//! Arclain Widgets - Reusable UI widget primitives
//!
//! This crate provides atomic UI widgets for the Arclain application:
//! - `Text` - Pixel-aligned text with theme support
//! - `TextButton` - Text button with semantic variants
//! - `TextInput` - Styled single-line text input
//! - `IconButton` - Icon-only button
//! - `ToggleButton` - Button with selected/unselected state
//! - `ToggleSwitch` - On/off switch
//! - `SegmentedControl` - Two-option text selector
//! - `ThemedSlider` - Themed slider with value label
//! - `Chip` - Pill-shaped label/badge
//! - `ThemedDropdown` - Themed ComboBox wrapper
//! - `SelectableChip` - Interactive chip with selection states
//! - `CollapsibleSection` - Collapsible panel with theme support
//! - `Toast` / `Toaster` - Toast notification system

pub mod button;
pub mod chips;
pub mod collapsible_section;
// `debug` module is dev-only — the paint_*_debug overlays are
// instrumentation helpers with no external consumers and shouldn't
// stay in release binaries.
#[cfg(debug_assertions)]
pub mod debug;
pub mod dropdown;
pub mod icon_button;
pub mod segmented_control;
pub mod selectable_chip;
pub mod text;
pub mod text_input;
pub mod text_layout;
pub mod themed_slider;
pub mod toast;
pub mod toggle_button;
pub mod toggle_switch;

pub use button::{ButtonSize, TextButton};
pub use chips::Chips;
pub use collapsible_section::CollapsibleSection;
#[cfg(debug_assertions)]
pub use debug::{
    paint_centering_debug, paint_child_in_parent_debug, paint_text_centering_debug,
    paint_widget_rect_debug, ui_debug_guidelines_enabled,
};
pub use dropdown::ThemedDropdown;
pub use icon_button::{IconButton, IconButtonSize};
pub use segmented_control::SegmentedControl;
pub use selectable_chip::SelectableChip;
pub use text::{get_theme, pixel_align, set_theme, Text};
pub use text_input::{TextInput, TextInputSize, TextInputState, TextInputResponse, SlotContent};
pub use text_layout::{
    layout_text_visually_centered, paint_text_left_in_rect_visually_centered,
    paint_text_visually_centered,
};
pub use themed_slider::ThemedSlider;
pub use toast::{Toast, ToastLevel, Toaster};
pub use toggle_button::ToggleButton;
pub use toggle_switch::ToggleSwitch;
