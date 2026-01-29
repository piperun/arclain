//! Core types for metastore metadata library
//!
//! This crate re-exports types from gameta_core for backward compatibility.
//! New code should depend on gameta_core directly.

// Re-export all types from gameta_core
pub use gameta_core::{
    ContentReference, ContentType, MetadataSource, ProductMetadata, SearchResult,
};
