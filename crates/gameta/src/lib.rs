//! gameta - Game metadata server/client
//!
//! This crate provides HTTP fetching, caching, database storage, and API serving
//! for game metadata. It uses `gameta_lib` for parsing.
//!
//! # Status
//! This is currently a stub. Implementation coming soon.
//!
//! # Planned Features
//! - HTTP client for fetching metadata from DLSite, Steam, etc.
//! - Local caching (memory and disk)
//! - Database storage (SQLite, Postgres)
//! - REST API server for external access
//! - Client SDK

// Re-export gameta_lib for convenience
pub use gameta_lib;

// TODO: Implement HTTP fetching
// pub mod http;

// TODO: Implement caching
// pub mod cache;

// TODO: Implement database storage
// pub mod database;

// TODO: Implement API server
// pub mod api;

/// Placeholder for future high-level fetch function
pub fn fetch_metadata(_source: &str, _id: &str) -> Option<gameta_lib::ProductMetadata> {
    // TODO: Implement actual fetching
    None
}
