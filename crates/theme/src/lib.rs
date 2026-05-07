//! Arclain Theme - Semantic theming system for the Arclain application
//!
//! This crate provides:
//! - `ThemeColors` - Semantic color definitions (Flutter ColorScheme pattern)
//! - `ThemeExtensions` - Optional helper colors (file types, badges)
//! - `AppTheme` - Main theme holder with light/dark mode support
//! - `ButtonVariant` - Semantic button styling variants
//! - `load_cjk_fonts` - CJK font loading utilities
//!
//! # Color Hierarchy
//! - **Tier 1**: Core semantic colors (primary, secondary, tertiary, error)
//! - **Tier 2**: Surface colors (surface, on_surface, outline)
//! - **Tier 3**: Status colors (warning, success, info)
//! - **Tier 4**: Extensions (file types, badges)
//!
//! # Example
//! ```ignore
//! use arclain_theme::{AppTheme, ThemeColors};
//!
//! // Create from seed colors
//! let colors = ThemeColors::from_seed(primary, secondary, tertiary, surface, true);
//!
//! // Use preset themes
//! let theme = AppTheme::new(true); // dark mode
//! ```

mod colors;
mod extensions;
mod fonts;
pub mod spacing;
mod theme;
pub mod themes;
mod variants;

// Re-export all public types
pub use colors::ThemeColors;
pub use extensions::ThemeExtensions;
pub use fonts::load_cjk_fonts;
pub use theme::AppTheme;
pub use variants::ButtonVariant;
