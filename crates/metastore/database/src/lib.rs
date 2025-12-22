//! Database backends for metastore
//!
//! Provides SQLite implementation of the StorageBackend trait.

mod sqlite;

pub use sqlite::SqliteBackend;
