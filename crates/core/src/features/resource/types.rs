//! Resource types for data management

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How data should be stored
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageStrategy {
    /// Use ContentCache (default, persistent)
    Cache,
    /// Store to disk directly
    Disk,
    /// Keep in memory only (ephemeral)
    Memory,
    /// Don't store, re-fetch every time
    NoStore,
}

impl Default for StorageStrategy {
    fn default() -> Self {
        Self::Cache
    }
}

/// Type of resource being managed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceType {
    /// Binary image data
    Image,
    /// Structured metadata (JSON)
    Metadata,
    /// Generic binary data
    Binary,
    /// Text content
    Text,
}

/// Configuration for the ResourceManager
#[derive(Debug, Clone)]
pub struct ResourceConfig {
    /// Default storage strategy
    pub default_strategy: StorageStrategy,
    /// Fallback directory for disk storage
    pub fallback_dir: Option<PathBuf>,
    /// Whether caching is enabled globally
    pub caching_enabled: bool,
    /// Maximum cached resource size (bytes)
    pub max_resource_size: Option<usize>,
}

impl Default for ResourceConfig {
    fn default() -> Self {
        Self {
            default_strategy: StorageStrategy::Cache,
            fallback_dir: None,
            caching_enabled: true,
            max_resource_size: Some(50 * 1024 * 1024), // 50MB default
        }
    }
}

/// Request to fetch a resource
#[derive(Debug, Clone)]
pub struct ResourceRequest {
    /// Unique key for this resource
    pub key: String,
    /// Source URL to fetch from (if network)
    pub url: Option<String>,
    /// Source file path (if local)
    pub path: Option<PathBuf>,
    /// Type of resource
    pub resource_type: ResourceType,
    /// Associated product ID (for organization)
    pub product_id: Option<String>,
    /// Override storage strategy
    pub storage_override: Option<StorageStrategy>,
}

impl ResourceRequest {
    /// Create a request for a network resource
    pub fn from_url(key: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            url: Some(url.into()),
            path: None,
            resource_type: ResourceType::Binary,
            product_id: None,
            storage_override: None,
        }
    }

    /// Create a request for a local file
    pub fn from_path(key: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            key: key.into(),
            url: None,
            path: Some(path.into()),
            resource_type: ResourceType::Binary,
            product_id: None,
            storage_override: None,
        }
    }

    /// Set resource type
    pub fn with_type(mut self, resource_type: ResourceType) -> Self {
        self.resource_type = resource_type;
        self
    }

    /// Set product ID
    pub fn with_product(mut self, product_id: impl Into<String>) -> Self {
        self.product_id = Some(product_id.into());
        self
    }

    /// Override storage strategy
    pub fn with_storage(mut self, strategy: StorageStrategy) -> Self {
        self.storage_override = Some(strategy);
        self
    }
}

/// Status of a resource fetch operation
#[derive(Debug, Clone)]
pub enum ResourceStatus {
    /// Already cached, no fetch needed
    Cached,
    /// Fetch in progress
    Fetching,
    /// Successfully fetched and stored
    Ready,
    /// Fetch failed
    Failed(String),
}

impl ResourceStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, ResourceStatus::Cached | ResourceStatus::Ready)
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, ResourceStatus::Failed(_))
    }
}

/// Result of getting a resource
#[derive(Debug, Clone)]
pub struct ResourceData {
    /// The raw data
    pub data: Vec<u8>,
    /// Content type if known
    pub content_type: Option<String>,
    /// Where the data came from
    pub source: ResourceSource,
}

/// Where resource data came from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceSource {
    /// From ContentCache
    Cache,
    /// From disk
    Disk,
    /// From network (freshly fetched)
    Network,
    /// From memory
    Memory,
}
