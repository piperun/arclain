//! Metadata providers for various platforms

mod provider_trait;

#[cfg(feature = "dlsite")]
pub mod dlsite;

#[cfg(feature = "itchio")]
pub mod itchio;

#[cfg(feature = "steam")]
pub mod steam;

pub use provider_trait::*;
