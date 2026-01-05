//! Minimal SQLite ORM
//!
//! Provides:
//! - `SqliteDb` wrapper with connection pooling
//! - `#[derive(DbConfig)]` for single-row config tables
//! - Type-safe query builders (SELECT, INSERT, UPDATE, DELETE)
//!
//! # Type-Safe API (Recommended)
//!
//! ```ignore
//! #[derive(DbTable)]
//! #[table = "users"]
//! struct Users {
//!     id: i32,
//!     name: String,
//! }
//!
//! let query = Select::from(Users)
//!     .filter(Users::name.equal("John"))
//!     .order(&Users::id, Order::Asc);
//! ```

// Type-safe modules
mod delete;
mod insert;
mod select;
mod typed;
mod update;

// Legacy modules (deprecated - use typed API)
mod mutation_builders;
mod query_builder;
mod sqlite_db;

#[cfg(test)]
mod comprehensive_tests;

// Re-export derive macro
pub use mini_orm_derive::DbConfig;

// Type-safe API exports
pub use delete::Delete;
pub use insert::{Conflict, Insert};
pub use select::{Join, Select};
pub use typed::{Column, ColumnId, ColumnRef, Expr, JoinOn, Order, Table, TableId, Value};
pub use update::Update;

// Connection management
pub use rusqlite::{Connection, Row};
pub use sqlite_db::SqliteDb;

// Legacy exports (deprecated)
#[deprecated(since = "0.2.0", note = "Use typed Insert/Update/Delete instead")]
pub use mutation_builders::{DeleteBuilder, InsertBuilder, OnConflict, UpdateBuilder};
#[deprecated(since = "0.2.0", note = "Use typed Select instead")]
pub use query_builder::{JoinType, OrderDirection, QueryBuilder};
