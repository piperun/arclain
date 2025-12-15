//! Arclain Theme - Semantic theming system for the Arclain application
//!
//! This crate provides:
//! - `ThemeColors` - Semantic color definitions (Material-like naming)
//! - `AppTheme` - Main theme holder with light/dark mode support
//! - `ButtonVariant` - Semantic button styling variants
//! - `load_cjk_fonts` - CJK font loading utilities
//!
//! # Example
//! ```ignore
//! use arclain_theme::{AppTheme, ButtonVariant, ThemeColors};
//!
//! let theme = AppTheme::new(true); // dark mode
//! let colors = &theme.colors;
//!
//! let button_fill = ButtonVariant::Primary.bg_color(colors);
//! ```

mod colors;
mod fonts;
mod theme;
pub mod themes;
mod variants;

// Re-export all public types
pub use colors::ThemeColors;
pub use fonts::load_cjk_fonts;
pub use theme::AppTheme;
pub use variants::ButtonVariant;
