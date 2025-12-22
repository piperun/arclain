//! Data source resolver module
//!
//! Provides a trait-based architecture for resolving data from various sources.
//! Each source (cache, network, file, etc.) implements the `DataSourceResolver` trait.

mod content;
mod memory;
mod metadata;
mod network;
mod trait_def;

pub use content::ContentCacheResolver;
pub use memory::MemoryResolver;
pub use metadata::MetadataStoreResolver;
pub use network::NetworkResolver;
pub use trait_def::{DataSourceResolver, ResolveError};
