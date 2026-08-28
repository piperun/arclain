//! arclain-data: Unified data access layer
//!
//! Provides data management functionality with:
//! - Content caching (images, binary data)
//! - Resource management (network, disk, memory storage)
//! - Plugin/UI facing API with modular data resolution
//! - Trait-based resolver pattern for extensibility

pub mod features;
pub mod shared;
pub mod traits;

pub use traits::CacheIndex;
#[cfg(feature = "gameta")]
pub use traits::MetadataReader;

// Re-export main types at crate root
pub use features::api::{
    DataRequest, DataResult, DataService, DataSource, DataStatus, SourceChain,
};
pub use features::content_cache::{CacheCapacityRefusal, CacheLimits, CacheOwner, ContentCache};
#[cfg(feature = "gameta")]
pub use features::resolver::MetadataStoreResolver;
pub use features::resolver::{
    ContentCacheResolver, DataSourceResolver, NetworkResolver, ResolveError, ServerResolver,
};
pub use features::streaming_download::fetch_url_to_cache;
// MemoryResolver is intentionally not re-exported — it's only used by
// resolver-internal tests inside `features/resolver/memory.rs`.
pub use features::resource_manager::ResourceManager;
pub use shared::{
    ResourceConfig, ResourceData, ResourceRequest, ResourceSource, ResourceStatus, ResourceType,
    StorageStrategy, DEFAULT_MAX_RESOURCE_SIZE_BYTES,
};

// `IndexSet` is exposed by `SourceChain` in our public API; re-export
// so callers building chains don't need a separate `indexmap` dep.
// (`anyhow::Result` was previously re-exported too — dropped because
// it polluted callers' `Result` namespace and had zero consumers.)
pub use indexmap::IndexSet;
