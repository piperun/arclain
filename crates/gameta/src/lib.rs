//! gameta - Game metadata server/client
//!
//! This crate provides HTTP fetching, caching, database storage, and API serving
//! for game metadata. It uses `gameta_core` for types/traits and `gameta_lib` for parsing.
//!
//! # Status
//! This is currently a stub. Implementation coming soon.
//! This crate will eventually become `gameta_server` - the optional daemon.
//!
//! # Planned Features
//! - HTTP client for fetching metadata from DLSite, Steam, etc.
//! - Local caching (memory and disk)
//! - Database storage (SQLite, Postgres)
//! - REST API server for external access
//! - Client SDK

// Re-export core types and traits for convenience
pub use gameta_core::{
    ContentReference, ContentType, HttpClient, HttpRequest, HttpResponse, MetadataProvider,
    MetadataSource, ParseError, ProductMetadata, SearchResult, StorageBackend, StorageError,
};

// Re-export gameta_lib for parsing functions
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
pub fn fetch_metadata(_source: &str, _id: &str) -> Option<ProductMetadata> {
    // TODO: Implement actual fetching
    None
}
