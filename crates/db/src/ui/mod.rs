//! UI configuration feature module
//!
//! Split into four cohesive sub-modules (audit fat-file callout —
//! `config.rs` was a 766-LOC mix of types, schema, rusqlite CRUD,
//! seed data, and Diesel DSL accessors):
//!
//! - [`types`]      — `DisplayMode`, `ActionType`, `UiRegion`, `UiItem`,
//!                    `UiRegionConfig`. Pure data, no DB dependency.
//! - [`config`]     — schema constants + rusqlite CRUD. Owned by callers
//!                    that hold a `rusqlite::Connection`.
//! - [`diesel_ops`] — Diesel DSL mirror of the rusqlite functions.
//! - [`seed`]       — canonical default toolbar / context-menu / panel
//!                    items and display options.

pub mod config;
pub mod diesel_ops;
pub mod seed;
pub mod types;

pub use config::*;
pub use diesel_ops::*;
pub use seed::*;
pub use types::*;
