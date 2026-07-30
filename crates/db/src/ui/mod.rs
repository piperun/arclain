//! UI configuration feature module
//!
//! Split into four cohesive sub-modules (audit fat-file callout —
//! `config.rs` was a 766-LOC mix of types, schema, rusqlite CRUD,
//! seed data, and Diesel DSL accessors):
//!
//! - [`types`]      — `DisplayMode`, `ActionType`, `UiRegion`, `UiItem`,
//!                    `UiRegionConfig`. Pure data, no DB dependency.
//! - [`config`]     — schema constants + the two rusqlite seed helpers
//!                    (`upsert_item`, `set_display_option`) called from
//!                    `ConfigDb::create_tables` before the Diesel pool
//!                    exists. Everything else uses the Diesel mirror.
//! - [`diesel_ops`] — Diesel DSL CRUD. Used by
//!                    `core::services::ui_service` over a `DieselPool`.
//! - [`seed`]       — canonical default toolbar / context-menu / panel
//!                    items and display options. Uses `config`.

pub mod config;
pub mod diesel_ops;
pub mod seed;
pub mod types;

// Startup-path API (rusqlite). The Diesel mirrors of `upsert_item` and
// `set_display_option` are reached via `crate::ui::diesel_ops::*` (and
// re-exported from `crate` as the bare names).
pub use config::ensure_ui_tables;

// Diesel DSL CRUD — the canonical API for all post-startup callers.
pub use diesel_ops::{
    delete_item, get_display_option, list_items_by_region, set_display_option, set_display_options,
    sync_host_item, upsert_item,
};

pub use seed::seed_defaults_if_empty;
pub use types::*;
