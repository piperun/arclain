//! gameta_core - Core types and traits for game metadata
//!
//! This crate provides the foundational types and trait definitions used by:
//! - `gameta_lib` - The embeddable metadata library
//! - `gameta_server` - The optional metadata daemon
//!
//! # Types
//! - `ProductMetadata` - Unified metadata structure for all platforms
//! - `MetadataSource` - Platform identifier (DLSite, Steam, etc.)
//! - `ContentReference` - Reference to cached binary assets
//! - `SearchResult` - Search result from a provider
//!
//! # Traits
//! - `MetadataProvider` - Interface for platform-specific providers
//! - `StorageBackend` - Interface for metadata storage
//! - `HttpClient` - Interface for HTTP execution

pub mod types;
pub mod traits;
pub mod errors;

// Re-export commonly used types at crate root
pub use types::{
    ContentReference, ContentType, MetadataSource, ProductMetadata, SearchResult,
};
pub use traits::{HttpClient, HttpMethod, HttpRequest, HttpResponse, MetadataProvider, StorageBackend};
pub use errors::{HttpError, ParseError, StorageError};
