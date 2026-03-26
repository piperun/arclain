//! arclain-data: Unified data access layer
//!
//! Provides data management functionality with:
//! - Content caching (images, binary data)
//! - Resource management (network, disk, memory storage)
//! - Plugin/UI facing API with modular data resolution
//! - Trait-based resolver pattern for extensibility

pub mod features;
pub mod shared;

// Re-export main types at crate root
pub use features::api::{
    DataRequest, DataResult, DataService, DataSource, DataStatus, SourceChain,
};
pub use features::content_cache::ContentCache;
pub use features::resolver::{
    ContentCacheResolver, DataSourceResolver, MemoryResolver, MetadataStoreResolver,
    NetworkResolver, ResolveError, ServerResolver,
};
pub use features::resource_manager::ResourceManager;
pub use shared::{
    ResourceConfig, ResourceData, ResourceRequest, ResourceSource, ResourceStatus, ResourceType,
    StorageStrategy,
};

// Re-export common types
pub use anyhow::Result;
pub use indexmap::IndexSet;
