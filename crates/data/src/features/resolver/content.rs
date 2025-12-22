//! ContentCache resolver
//!
//! Resolves binary content from cacache (via ResourceManager).

use super::{DataSourceResolver, ResolveError};
use crate::features::api::DataRequest;
use crate::features::resource_manager::ResourceManager;
use crate::shared::{ResourceRequest, ResourceType};
use std::sync::Arc;

/// Resolver for binary content stored in cacache
pub struct ContentCacheResolver {
    manager: Arc<ResourceManager>,
}

impl ContentCacheResolver {
    pub fn new(manager: Arc<ResourceManager>) -> Self {
        Self { manager }
    }
}

impl DataSourceResolver for ContentCacheResolver {
    fn try_resolve(&self, key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
        tracing::info!("[ContentCacheResolver] Looking for key: {}", key);
        match self.manager.get(key) {
            Some(data) => {
                tracing::info!(
                    "[ContentCacheResolver] FOUND {} in cache ({} bytes)",
                    key,
                    data.data.len()
                );
                Ok(data.data)
            }
            None => {
                tracing::info!("[ContentCacheResolver] NOT FOUND: {}", key);
                Err(ResolveError::NotFound)
            }
        }
    }

    fn try_store(&self, key: &str, data: &[u8], request: &DataRequest) -> Result<(), ResolveError> {
        let req = ResourceRequest::from_url(key, request.url.as_deref().unwrap_or(""))
            .with_type(ResourceType::Binary)
            .with_product(request.product_id.as_deref().unwrap_or(""));

        self.manager
            .put(key, data, &req)
            .map_err(|e| ResolveError::IoError(e))
    }

    fn has(&self, key: &str, _request: &DataRequest) -> bool {
        self.manager.has(key)
    }
}
