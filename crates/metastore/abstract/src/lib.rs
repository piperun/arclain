//! Abstraction traits for metastore
//!
//! This crate re-exports traits from gameta_core for backward compatibility.
//! New code should depend on gameta_core directly.

// Re-export all traits and types from gameta_core
pub use gameta_core::{
    HttpClient, HttpError, HttpMethod, HttpRequest, HttpResponse, MetadataProvider, ParseError,
    StorageBackend, StorageError,
};

// Re-export types for backward compatibility
pub use gameta_core::{ContentReference, MetadataSource, ProductMetadata, SearchResult};
