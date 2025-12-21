//! Resource Manager - orchestrates data fetching and storage
//!
//! Provides a unified interface for getting data from network, disk, or cache
//! without exposing storage details to consumers.

use super::types::{
    ResourceConfig, ResourceData, ResourceRequest, ResourceSource, ResourceType, StorageStrategy,
};
use crate::utilities::ContentCache;
use arclain_db::CacheType;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info};

/// Manages resource fetching, caching, and retrieval
pub struct ResourceManager {
    /// Content cache (optional - can be disabled)
    cache: Option<Arc<ContentCache>>,
    /// Fallback disk directory
    fallback_dir: Option<PathBuf>,
    /// In-memory store for ephemeral resources
    memory_store: RwLock<HashMap<String, Vec<u8>>>,
    /// Configuration
    config: ResourceConfig,
}

impl ResourceManager {
    /// Create a new ResourceManager with cache
    pub fn new(cache: Arc<ContentCache>, config: ResourceConfig) -> Self {
        Self {
            cache: Some(cache),
            fallback_dir: config.fallback_dir.clone(),
            memory_store: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Create a ResourceManager without cache (disk/memory only)
    pub fn without_cache(config: ResourceConfig) -> Self {
        Self {
            cache: None,
            fallback_dir: config.fallback_dir.clone(),
            memory_store: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Check if a resource exists (in any storage)
    pub fn has(&self, key: &str) -> bool {
        // Check memory first
        if self.memory_store.read().contains_key(key) {
            return true;
        }

        // Check cache
        if let Some(cache) = &self.cache {
            if cache.has(key).unwrap_or(false) {
                return true;
            }
        }

        // Check disk fallback
        if let Some(dir) = &self.fallback_dir {
            let path = dir.join(sanitize_key(key));
            if path.exists() {
                return true;
            }
        }

        false
    }

    /// Get a resource synchronously (from storage only, no fetch)
    pub fn get(&self, key: &str) -> Option<ResourceData> {
        // Try memory first
        if let Some(data) = self.memory_store.read().get(key).cloned() {
            return Some(ResourceData {
                data,
                content_type: None,
                source: ResourceSource::Memory,
            });
        }

        // Try cache
        if let Some(cache) = &self.cache {
            if let Ok(Some(data)) = cache.get(key) {
                return Some(ResourceData {
                    data,
                    content_type: None,
                    source: ResourceSource::Cache,
                });
            }
        }

        // Try disk fallback
        if let Some(dir) = &self.fallback_dir {
            let path = dir.join(sanitize_key(key));
            if let Ok(data) = std::fs::read(&path) {
                return Some(ResourceData {
                    data,
                    content_type: None,
                    source: ResourceSource::Disk,
                });
            }
        }

        None
    }

    /// Store a resource directly (without fetching)
    pub fn put(&self, key: &str, data: &[u8], request: &ResourceRequest) -> Result<(), String> {
        let strategy = request
            .storage_override
            .unwrap_or(self.config.default_strategy);

        // Check size limit
        if let Some(max) = self.config.max_resource_size {
            if data.len() > max {
                return Err(format!(
                    "Resource too large: {} bytes (max: {})",
                    data.len(),
                    max
                ));
            }
        }

        match strategy {
            StorageStrategy::Cache => {
                if let Some(cache) = &self.cache {
                    let cache_type = match request.resource_type {
                        ResourceType::Image => CacheType::Screenshot,
                        ResourceType::Metadata => CacheType::Metadata,
                        _ => CacheType::Screenshot,
                    };
                    cache
                        .put(
                            key,
                            data,
                            cache_type,
                            request.product_id.as_deref(),
                            request.url.as_deref(), // source_url
                        )
                        .map_err(|e| e.to_string())?;
                    debug!("Stored {} bytes to cache: {}", data.len(), key);
                } else {
                    // Fallback to disk if cache disabled
                    self.store_to_disk(key, data)?;
                }
            }
            StorageStrategy::Disk => {
                self.store_to_disk(key, data)?;
            }
            StorageStrategy::Memory => {
                self.memory_store
                    .write()
                    .insert(key.to_string(), data.to_vec());
                debug!("Stored {} bytes to memory: {}", data.len(), key);
            }
            StorageStrategy::NoStore => {
                debug!("NoStore strategy - not storing: {}", key);
            }
        }

        Ok(())
    }

    /// Store to disk fallback
    fn store_to_disk(&self, key: &str, data: &[u8]) -> Result<(), String> {
        let dir = self
            .fallback_dir
            .as_ref()
            .ok_or_else(|| "No fallback directory configured".to_string())?;

        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

        let path = dir.join(sanitize_key(key));
        std::fs::write(&path, data).map_err(|e| e.to_string())?;

        debug!("Stored {} bytes to disk: {}", data.len(), path.display());
        Ok(())
    }

    /// Fetch and store a resource (blocking)
    ///
    /// Note: For async fetching, use request_async + poll_status pattern
    pub fn fetch_sync(
        &self,
        request: &ResourceRequest,
        http_get: impl Fn(&str) -> Result<Vec<u8>, String>,
    ) -> Result<ResourceData, String> {
        let key = &request.key;

        // Check if already cached
        if let Some(data) = self.get(key) {
            info!("Resource found in storage: {}", key);
            return Ok(data);
        }

        // Fetch from source
        let data = if let Some(url) = &request.url {
            info!("Fetching resource from network: {}", url);
            http_get(url)?
        } else if let Some(path) = &request.path {
            info!("Reading resource from disk: {}", path.display());
            std::fs::read(path).map_err(|e| e.to_string())?
        } else {
            return Err("No URL or path provided for resource".to_string());
        };

        // Store it
        self.put(key, &data, request)?;

        Ok(ResourceData {
            data,
            content_type: None,
            source: ResourceSource::Network,
        })
    }

    /// Delete a resource from disk storage only
    /// Note: ContentCache doesn't expose a delete method currently
    pub fn delete_from_disk(&self, key: &str) -> Result<(), String> {
        // Remove from memory
        self.memory_store.write().remove(key);

        // Remove from disk
        if let Some(dir) = &self.fallback_dir {
            let path = dir.join(sanitize_key(key));
            let _ = std::fs::remove_file(path);
        }

        Ok(())
    }

    /// Clear all memory-stored resources
    pub fn clear_memory(&self) {
        self.memory_store.write().clear();
    }

    /// Get current config
    pub fn config(&self) -> &ResourceConfig {
        &self.config
    }

    /// Update config
    pub fn set_config(&mut self, config: ResourceConfig) {
        self.fallback_dir = config.fallback_dir.clone();
        self.config = config;
    }

    /// Check if caching is enabled
    pub fn is_cache_enabled(&self) -> bool {
        self.cache.is_some() && self.config.caching_enabled
    }
}

/// Sanitize a key for use as a filename
fn sanitize_key(key: &str) -> String {
    key.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_storage() {
        let config = ResourceConfig {
            default_strategy: StorageStrategy::Memory,
            ..Default::default()
        };
        let manager = ResourceManager::without_cache(config);

        let request = ResourceRequest::from_url("test-key", "http://example.com")
            .with_storage(StorageStrategy::Memory);

        // Store directly
        manager.put("test-key", b"hello world", &request).unwrap();

        // Retrieve
        let data = manager.get("test-key").unwrap();
        assert_eq!(data.data, b"hello world");
        assert_eq!(data.source, ResourceSource::Memory);
    }

    #[test]
    fn test_has() {
        let manager = ResourceManager::without_cache(ResourceConfig::default());

        assert!(!manager.has("nonexistent"));

        manager
            .memory_store
            .write()
            .insert("exists".to_string(), vec![1, 2, 3]);

        assert!(manager.has("exists"));
    }
}
