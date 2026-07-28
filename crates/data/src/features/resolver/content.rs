//! ContentCache resolver
//!
//! Resolves binary content from cacache (via ResourceManager).

use super::{default_materialization_limit, DataSourceResolver, ResolveError};
use crate::features::api::DataRequest;
use crate::features::content_cache::CacheOwner;
use crate::features::resource_manager::ResourceManager;
use crate::shared::{safe_log_fingerprint, ResourceRequest, ResourceType};
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
        self.try_resolve_with_limit(key, _request, default_materialization_limit())
    }

    fn try_resolve_with_limit(
        &self,
        key: &str,
        request: &DataRequest,
        limit: usize,
    ) -> Result<Vec<u8>, ResolveError> {
        tracing::debug!(
            "[ContentCacheResolver] Looking for key: {}",
            safe_log_fingerprint(key)
        );
        let owner = request
            .plugin_id
            .as_deref()
            .map(CacheOwner::plugin)
            .unwrap_or_else(CacheOwner::host);
        match self.manager.get_with_limit_for_owner(&owner, key, limit) {
            Some(data) => {
                tracing::debug!(
                    "[ContentCacheResolver] FOUND {} in cache ({} bytes)",
                    safe_log_fingerprint(key),
                    data.data.len()
                );
                Ok(data.data)
            }
            None => {
                tracing::debug!(
                    "[ContentCacheResolver] NOT FOUND: {}",
                    safe_log_fingerprint(key)
                );
                Err(ResolveError::NotFound)
            }
        }
    }

    fn try_store(&self, key: &str, data: &[u8], request: &DataRequest) -> Result<(), ResolveError> {
        // Only set product_id if it's actually present and non-empty
        let product_id = request.product_id.as_deref().filter(|s| !s.is_empty());

        let mut req = ResourceRequest::from_url(key, request.url.as_deref().unwrap_or(""))
            .with_type(ResourceType::Binary);

        if let Some(pid) = product_id {
            req = req.with_product(pid);
        }

        self.manager
            .put_for_owner(
                &request
                    .plugin_id
                    .as_deref()
                    .map(CacheOwner::plugin)
                    .unwrap_or_else(CacheOwner::host),
                key,
                data,
                &req,
            )
            .map_err(|e| ResolveError::IoError(e))
    }

    fn has(&self, key: &str, request: &DataRequest) -> bool {
        let owner = request
            .plugin_id
            .as_deref()
            .map(CacheOwner::plugin)
            .unwrap_or_else(CacheOwner::host);
        self.manager.has_for_owner(&owner, key)
    }

    fn has_with_limit(&self, key: &str, request: &DataRequest, _limit: usize) -> bool {
        self.has(key, request)
    }
}
