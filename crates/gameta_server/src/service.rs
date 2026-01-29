//! Metadata service - core business logic
//!
//! Handles:
//! - Fetching metadata from providers
//! - Caching responses
//! - Database storage
//! - Background workers

use gameta_core::{MetadataSource, ProductMetadata, SearchResult};
use std::sync::Arc;

use crate::config::ServerConfig;

/// The metadata service
pub struct MetadataService {
    config: ServerConfig,
    // TODO: Add database connection
    // TODO: Add cache
    // TODO: Add HTTP client
}

impl MetadataService {
    /// Create a new metadata service
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }

    /// Get metadata for a product
    pub async fn get_metadata(
        &self,
        _source: MetadataSource,
        _external_id: &str,
    ) -> anyhow::Result<Option<ProductMetadata>> {
        // TODO: Implement
        // 1. Check database
        // 2. If not found, check if fetch is in progress
        // 3. Return cached or None
        Ok(None)
    }

    /// Fetch metadata from source
    pub async fn fetch_metadata(
        &self,
        _source: MetadataSource,
        _external_id: &str,
        _force: bool,
    ) -> anyhow::Result<ProductMetadata> {
        // TODO: Implement
        // 1. Get provider for source
        // 2. Build HTTP requests
        // 3. Execute requests
        // 4. Parse responses
        // 5. Store in database
        // 6. Return metadata
        anyhow::bail!("Not implemented")
    }

    /// Search for products
    pub async fn search(
        &self,
        _query: &str,
        _source: Option<MetadataSource>,
    ) -> anyhow::Result<Vec<SearchResult>> {
        // TODO: Implement
        Ok(vec![])
    }

    /// Get service configuration
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }
}

/// Create a shared service instance
pub fn create_service(config: ServerConfig) -> Arc<MetadataService> {
    Arc::new(MetadataService::new(config))
}
