//! Data API types

use crate::shared::ResourceType;
use indexmap::IndexSet;
use std::path::PathBuf;

/// Individual data source
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataSource {
    /// Structured metadata stored in SQLite (arclain_db::MetadataStore)
    MetadataStore,
    /// Binary content stored in cacache (via ResourceManager)
    ContentCache,
    /// Local file system
    LocalFile,
    /// In-memory ephemeral store
    Memory,
    /// HTTP network fetch
    Network,
    /// Gameta server metadata API
    GametaServer,
}

/// Ordered set of data sources to try - no duplicates, preserves order
pub type SourceChain = IndexSet<DataSource>;

/// Status of a data request
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataStatus {
    Pending,
    Fetching,
    Ready,
    Failed,
    Cached,
}

/// Result of a data operation
#[derive(Debug, Clone)]
pub struct DataResult {
    pub status: DataStatus,
    pub data: Option<Vec<u8>>,
    pub error: Option<String>,
}

impl DataResult {
    /// Create a ready result with data
    pub fn ready(data: Vec<u8>) -> Self {
        Self {
            status: DataStatus::Ready,
            data: Some(data),
            error: None,
        }
    }

    /// Create a cached result with data
    pub fn cached(data: Vec<u8>) -> Self {
        Self {
            status: DataStatus::Cached,
            data: Some(data),
            error: None,
        }
    }

    /// Create a failed result with error message
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            status: DataStatus::Failed,
            data: None,
            error: Some(error.into()),
        }
    }

    /// Create a pending result
    pub fn pending() -> Self {
        Self {
            status: DataStatus::Pending,
            data: None,
            error: None,
        }
    }
}

/// Request for data
#[derive(Debug, Clone)]
pub struct DataRequest {
    /// Unique key for this data
    pub key: String,
    /// URL for network fetch (required if Network is in sources)
    pub url: Option<String>,
    /// Path for local file (required if LocalFile is in sources)
    pub path: Option<PathBuf>,
    /// Type of resource
    pub resource_type: ResourceType,
    /// Associated product ID (for organization/caching)
    pub product_id: Option<String>,
    /// Plugin ID making the request (for proxy routing)
    pub plugin_id: Option<String>,
    /// Resolution chain - sources to try in order
    /// Empty = use default chain
    pub sources: SourceChain,
}

impl DataRequest {
    /// Create a request with default source chain
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            url: None,
            path: None,
            resource_type: ResourceType::Binary,
            product_id: None,
            plugin_id: None,
            sources: IndexSet::new(),
        }
    }

    /// Set the URL for network fetch
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Set the file path for local file access
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Set the resource type
    pub fn with_type(mut self, resource_type: ResourceType) -> Self {
        self.resource_type = resource_type;
        self
    }

    /// Set the product ID
    pub fn with_product(mut self, product_id: impl Into<String>) -> Self {
        self.product_id = Some(product_id.into());
        self
    }

    /// Set the source chain
    pub fn with_sources(mut self, sources: impl IntoIterator<Item = DataSource>) -> Self {
        self.sources = sources.into_iter().collect();
        self
    }

    // === Common presets ===

    /// Cache only - for viewing cached entries (never fetches)
    pub fn cache_only(key: impl Into<String>) -> Self {
        let mut sources = IndexSet::new();
        sources.insert(DataSource::MetadataStore);
        sources.insert(DataSource::ContentCache);

        Self {
            key: key.into(),
            url: None,
            path: None,
            resource_type: ResourceType::Metadata,
            product_id: None,
            plugin_id: None,
            sources,
        }
    }

    /// Network only - force refresh (always fetches)
    pub fn network_only(key: impl Into<String>, url: impl Into<String>) -> Self {
        let mut sources = IndexSet::new();
        sources.insert(DataSource::Network);

        Self {
            key: key.into(),
            url: Some(url.into()),
            path: None,
            resource_type: ResourceType::Binary,
            product_id: None,
            plugin_id: None,
            sources,
        }
    }

    /// Cache first, then network (default fetch behavior)
    pub fn cache_first(key: impl Into<String>, url: impl Into<String>) -> Self {
        let mut sources = IndexSet::new();
        sources.insert(DataSource::ContentCache);
        sources.insert(DataSource::Network);

        Self {
            key: key.into(),
            url: Some(url.into()),
            path: None,
            resource_type: ResourceType::Binary,
            product_id: None,
            plugin_id: None,
            sources,
        }
    }

    /// Metadata cache first, then network
    pub fn metadata_first(key: impl Into<String>, url: impl Into<String>) -> Self {
        let mut sources = IndexSet::new();
        sources.insert(DataSource::MetadataStore);
        sources.insert(DataSource::Network);

        Self {
            key: key.into(),
            url: Some(url.into()),
            path: None,
            resource_type: ResourceType::Metadata,
            product_id: None,
            plugin_id: None,
            sources,
        }
    }

    /// Set the plugin ID for proxy routing
    pub fn with_plugin_id(mut self, plugin_id: impl Into<String>) -> Self {
        self.plugin_id = Some(plugin_id.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // DataResult
    // =========================================================================

    #[test]
    fn data_result_ready() {
        let r = DataResult::ready(vec![1, 2, 3]);
        assert_eq!(r.status, DataStatus::Ready);
        assert_eq!(r.data, Some(vec![1, 2, 3]));
        assert!(r.error.is_none());
    }

    #[test]
    fn data_result_cached() {
        let r = DataResult::cached(vec![10]);
        assert_eq!(r.status, DataStatus::Cached);
        assert!(r.data.is_some());
    }

    #[test]
    fn data_result_failed() {
        let r = DataResult::failed("timeout");
        assert_eq!(r.status, DataStatus::Failed);
        assert!(r.data.is_none());
        assert_eq!(r.error.as_deref(), Some("timeout"));
    }

    #[test]
    fn data_result_pending() {
        let r = DataResult::pending();
        assert_eq!(r.status, DataStatus::Pending);
        assert!(r.data.is_none());
        assert!(r.error.is_none());
    }

    // =========================================================================
    // DataRequest
    // =========================================================================

    #[test]
    fn data_request_new_defaults() {
        let req = DataRequest::new("test_key");
        assert_eq!(req.key, "test_key");
        assert!(req.url.is_none());
        assert!(req.path.is_none());
        assert_eq!(req.resource_type, ResourceType::Binary);
        assert!(req.sources.is_empty());
    }

    #[test]
    fn data_request_builder_chain() {
        let req = DataRequest::new("k")
            .with_url("https://example.com")
            .with_type(ResourceType::Image)
            .with_product("prod_123")
            .with_plugin_id("my_plugin");

        assert_eq!(req.url.as_deref(), Some("https://example.com"));
        assert_eq!(req.resource_type, ResourceType::Image);
        assert_eq!(req.product_id.as_deref(), Some("prod_123"));
        assert_eq!(req.plugin_id.as_deref(), Some("my_plugin"));
    }

    #[test]
    fn data_request_cache_only_preset() {
        let req = DataRequest::cache_only("my_key");
        assert_eq!(req.key, "my_key");
        assert!(req.url.is_none());
        assert!(req.sources.contains(&DataSource::MetadataStore));
        assert!(req.sources.contains(&DataSource::ContentCache));
        assert!(!req.sources.contains(&DataSource::Network));
    }

    #[test]
    fn data_request_network_only_preset() {
        let req = DataRequest::network_only("k", "https://api.test");
        assert_eq!(req.url.as_deref(), Some("https://api.test"));
        assert!(req.sources.contains(&DataSource::Network));
        assert_eq!(req.sources.len(), 1);
    }

    #[test]
    fn data_request_cache_first_preset() {
        let req = DataRequest::cache_first("k", "https://cdn.test/img.jpg");
        assert!(req.sources.contains(&DataSource::ContentCache));
        assert!(req.sources.contains(&DataSource::Network));
        assert_eq!(req.sources.len(), 2);
    }

    #[test]
    fn data_request_metadata_first_preset() {
        let req = DataRequest::metadata_first("k", "https://api.test/meta");
        assert!(req.sources.contains(&DataSource::MetadataStore));
        assert!(req.sources.contains(&DataSource::Network));
        assert_eq!(req.resource_type, ResourceType::Metadata);
    }

    #[test]
    fn data_request_with_sources() {
        let req = DataRequest::new("k")
            .with_sources([DataSource::Memory, DataSource::ContentCache]);
        assert_eq!(req.sources.len(), 2);
        assert!(req.sources.contains(&DataSource::Memory));
    }
}
