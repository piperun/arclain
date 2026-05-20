//! Data source resolver module
//!
//! Provides a trait-based architecture for resolving data from various sources.
//! Each source (cache, network, file, etc.) implements the `DataSourceResolver` trait.

mod content;
mod memory;
mod metadata;
mod network;
pub mod server;
mod trait_def;

pub use content::ContentCacheResolver;
// `memory::MemoryResolver` is intentionally not re-exported here —
// it's only constructed inside its own module's tests, so neither
// `arclain_data` nor downstream crates ever need it.
pub use metadata::MetadataStoreResolver;
pub use network::NetworkResolver;
pub use server::ServerResolver;
pub use trait_def::{DataSourceResolver, ResolveError};
