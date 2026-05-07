//! Semantic spacing constants
//!
//! Replaces ad-hoc `inner_margin(12.0)` / `inner_margin(8.0)` /
//! `inner_margin(20.0)` etc. scattered across the UI with a small set
//! of named values, so dialog-shell / card / row spacing stays
//! consistent and is changeable in one place (audit: `12.0` margin
//! reimplemented in 5+ places, 4 different "dialog content" paddings).
//!
//! Use as `Margin::same(Spacing::CARD)` or
//! `inner_margin(Spacing::CARD as f32)`.

/// Tight padding for compact rows and badge-like elements.
pub const COMPACT: i8 = 6;

/// Comfortable padding inside dense lists and rule rows.
pub const ROW: i8 = 8;

/// The dominant card / panel padding (12px) — used for settings
/// groups, plugin list items, organize panel header, etc.
pub const CARD: i8 = 12;

/// Section padding inside dialogs (form_dialog, password dialog body).
pub const SECTION: i8 = 16;

/// Outer dialog content padding — slightly looser than SECTION for the
/// dialog shell itself.
pub const DIALOG: i8 = 20;
