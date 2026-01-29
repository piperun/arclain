//! Metadata providers for various platforms
//!
//! Each provider implements the `MetadataProvider` trait from gameta_core,
//! handling platform-specific detection, fetching, and parsing.

#[cfg(feature = "dlsite")]
pub mod dlsite;

#[cfg(feature = "steam")]
pub mod steam;

#[cfg(feature = "itchio")]
pub mod itchio;
