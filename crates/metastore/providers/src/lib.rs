//! Metadata providers for various platforms
//!
//! This crate re-exports providers from gameta_lib for backward compatibility.
//! New code should depend on gameta_lib directly.

// Re-export provider trait and types from gameta_core
pub use gameta_core::{
    HttpRequest, HttpResponse, MetadataProvider, MetadataSource, ParseError, ProductMetadata,
    SearchResult,
};

// Re-export DLSite provider from gameta_lib
#[cfg(feature = "dlsite")]
pub use gameta_lib::providers::dlsite;

#[cfg(feature = "dlsite")]
pub use gameta_lib::providers::dlsite::DLSiteProvider;

// Stubs for future providers
#[cfg(feature = "itchio")]
pub mod itchio {
    //! itch.io metadata provider (skeleton)
}

#[cfg(feature = "steam")]
pub mod steam {
    //! Steam metadata provider (skeleton)
}
