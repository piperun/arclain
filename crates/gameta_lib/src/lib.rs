//! gameta_lib - Pure game metadata parsing library
//!
//! This crate provides parsing-only functionality for extracting metadata
//! from various game distribution platforms. It has NO I/O dependencies.
//!
//! # Supported Platforms
//! - DLSite (RJ/VJ/BJ codes)
//! - Steam (coming soon)
//! - itch.io (coming soon)
//!
//! # Usage
//! ```ignore
//! use gameta_lib::parsers::dlsite;
//!
//! // Parse DLSite data from raw responses
//! let metadata = dlsite::parse_dlsite("RJ123456", Some(api_json), Some(html))?;
//! ```

pub mod detect;
pub mod parsers;
pub mod providers;
pub mod urls;

// Re-export core types from gameta_core
pub use gameta_core::{MetadataSource, ParseError, ProductMetadata, SearchResult};

// Re-export other core types for convenience
pub use gameta_core::{ContentReference, ContentType};

// Re-export traits
pub use gameta_core::{HttpClient, HttpRequest, HttpResponse, MetadataProvider, StorageBackend};

// Convenience re-exports for the DLSite provider
#[cfg(feature = "dlsite")]
pub use providers::dlsite::DLSiteProvider;
