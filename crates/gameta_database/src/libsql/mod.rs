//! libSQL async storage backend
//!
//! This module provides an async database backend using libSQL,
//! which is a fork of SQLite with additional features like
//! replication, encryption at rest, and remote access.
//!
//! # Module Structure
//!
//! - [`backend`] - Core `LibSqlBackend` struct and constructors
//! - [`schema`] - Database schema initialization
//! - [`metadata`] - Product metadata CRUD operations
//! - [`content`] - Content reference storage
//! - [`fetch_log`] - Fetch logging for rate limiting
//! - [`integrity`] - Content integrity tracking (SRI hashes)

mod backend;
mod content;
mod fetch_log;
mod helpers;
mod integrity;
mod metadata;
mod schema;

pub use backend::LibSqlBackend;
