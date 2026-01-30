//! gameta_database - Database backends for gameta metadata storage
//!
//! This crate provides storage backend implementations for gameta:
//!
//! ## Features
//!
//! - `sync` (default) - Synchronous SQLite backend using rusqlite/mini-orm
//! - `async` - Asynchronous libSQL backend for use with tokio
//!
//! ## Backends
//!
//! - `SqliteBackend` - Sync SQLite storage (feature: sync)
//! - `LibSqlBackend` - Async libSQL storage (feature: async)
//!
//! ## Module Structure (async backend)
//!
//! The libSQL backend is organized into focused modules:
//! - `backend` - Core struct and constructors
//! - `schema` - Database schema initialization
//! - `metadata` - Product metadata CRUD
//! - `content` - Content reference storage
//! - `fetch_log` - Rate limiting and request logging
//! - `integrity` - SRI hash tracking for content verification

#[cfg(feature = "sync")]
mod sqlite;

#[cfg(feature = "async")]
mod libsql;

#[cfg(feature = "sync")]
pub use sqlite::SqliteBackend;

#[cfg(feature = "async")]
pub use libsql::LibSqlBackend;
