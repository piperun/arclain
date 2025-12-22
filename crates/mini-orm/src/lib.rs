//! Minimal SQLite ORM
//!
//! Provides:
//! - `SqliteDb` wrapper with connection pooling
//! - `#[derive(DbConfig)]` for single-row config tables
//! - Query builder helpers

mod sqlite_db;

pub use mini_orm_derive::DbConfig;
pub use rusqlite::{Connection, Row};
pub use sqlite_db::SqliteDb;
