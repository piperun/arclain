//! Unified search palette: a command-palette-style results dropdown
//! anchored under the header search box. One query searches open tabs
//! (by code/title/maker/file) and the active archive's entry paths;
//! activating a result switches tab or navigates to the file.
//!
//! - [`model`] — pure matching/ranking/highlight logic (no egui).
//! - [`view`] — egui rendering + keyboard interaction.

pub mod model;
pub mod view;

pub use model::{build_hits, match_range, SearchHit, TabSummary, MAX_FILE_HITS};
pub use view::{action_for, handle_keys, render_area, KeyIntent, SearchPaletteAction, SearchPaletteState};
