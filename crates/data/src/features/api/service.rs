//! Data API Service
//!
//! Provides the unified data access layer using the resolver pattern.
//! Plugins and UI request data, the service resolves it from the appropriate source.

use super::types::{DataRequest, DataResult, DataSource, DataStatus, SourceChain};
use crate::features::resolver::DataSourceResolver;
use indexmap::IndexSet;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

/// The main Data Service
///
/// Orchestrates data resolution using a registry of source resolvers.
#[derive(Clone)]
pub struct DataService {
    /// Registry of resolvers by source type
    resolvers: Arc<RwLock<HashMap<DataSource, Arc<dyn DataSourceResolver>>>>,
    /// Default source chain when request doesn't specify
    default_chain: SourceChain,
    /// State for pending requests (for async compatibility)
    state: Arc<DataApiState>,
    /// Service ID for logging context
    id: String,
}

/// Internal state for pending requests
struct DataApiState {
    pending: RwLock<HashMap<String, PendingRequest>>,
    next_id: RwLock<u64>,
}

struct PendingRequest {
    #[allow(dead_code)] // Kept for debugging/logging
    request: DataRequest,
    result: Option<DataResult>,
}

impl DataApiState {
    fn new() -> Self {
        Self {
            pending: RwLock::new(HashMap::new()),
            next_id: RwLock::new(0),
        }
    }

    fn generate_id(&self) -> String {
        let mut id = self.next_id.write();
        *id += 1;
        format!("data-{}", *id)
    }
}

impl DataService {
    pub fn new() -> Self {
        // Default chain: ContentCache first, then Network
        let mut default_chain = IndexSet::new();
        default_chain.insert(DataSource::ContentCache);
        default_chain.insert(DataSource::Network);

        Self {
            resolvers: Arc::new(RwLock::new(HashMap::new())),
            default_chain,
            state: Arc::new(DataApiState::new()),
            id: "global".to_string(),
        }
    }

    /// Set the service ID (builder pattern)
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Register a resolver for a data source
    pub fn register_resolver(&self, source: DataSource, resolver: Arc<dyn DataSourceResolver>) {
        info!(
            "[DataService:{}] Registered resolver for {:?}",
            self.id, source
        );
        self.resolvers.write().insert(source, resolver);
    }

    /// Set the default source chain
    pub fn set_default_chain(&mut self, chain: SourceChain) {
        self.default_chain = chain;
    }

    /// Save data to a specific source
    pub fn save_data(
        &self,
        source: DataSource,
        key: &str,
        data: &[u8],
    ) -> Result<(), crate::features::resolver::ResolveError> {
        let resolvers = self.resolvers.read();
        if let Some(resolver) = resolvers.get(&source) {
            // Create a dummy request for the context
            let request = DataRequest::new(key);
            resolver.try_store(key, data, &request)
        } else {
            Err(crate::features::resolver::ResolveError::NotConfigured)
        }
    }

    /// Resolve data synchronously using the source chain
    pub fn resolve(&self, request: &DataRequest) -> DataResult {
        let sources = if request.sources.is_empty() {
            &self.default_chain
        } else {
            &request.sources
        };

        debug!(
            "[DataService] Resolving '{}' using sources: {:?}",
            request.key, sources
        );

        let resolvers = self.resolvers.read();

        for source in sources {
            if let Some(resolver) = resolvers.get(source) {
                match resolver.try_resolve(&request.key, request) {
                    Ok(data) => {
                        info!(
                            "[DataService] Resolved '{}' from {:?} ({} bytes)",
                            request.key,
                            source,
                            data.len()
                        );

                        // If data came from network, try to store to earlier sources
                        if *source == DataSource::Network {
                            self.store_to_caches(&request.key, &data, request, sources);
                        }

                        return DataResult::ready(data);
                    }
                    Err(e) => {
                        debug!(
                            "[DataService] Source {:?} failed for '{}': {}",
                            source, request.key, e
                        );
                        continue;
                    }
                }
            } else {
                debug!("[DataService] No resolver registered for {:?}", source);
            }
        }

        DataResult::failed(format!(
            "No source could provide data for '{}'",
            request.key
        ))
    }

    /// Try to store data to cache sources in the chain
    fn store_to_caches(
        &self,
        key: &str,
        data: &[u8],
        request: &DataRequest,
        sources: &SourceChain,
    ) {
        // Don't cache search results - they're ephemeral and change over time
        if key.contains(":search:") {
            debug!("[DataService] Skipping cache for search key: {}", key);
            return;
        }

        let resolvers = self.resolvers.read();

        for source in sources {
            // Only store to cache sources, not network
            if *source == DataSource::Network {
                continue;
            }

            if let Some(resolver) = resolvers.get(source) {
                if let Err(e) = resolver.try_store(key, data, request) {
                    debug!(
                        "[DataService] Failed to store '{}' to {:?}: {}",
                        key, source, e
                    );
                } else {
                    debug!("[DataService] Stored '{}' to {:?}", key, source);
                }
            }
        }
    }

    // === Legacy API compatibility ===
    // These methods maintain backwards compatibility with the old request/poll pattern

    /// Request data (async pattern) - returns request ID
    pub fn request_data(&self, request: DataRequest) -> String {
        let id = self.state.generate_id();

        // Resolve immediately (we're sync for now)
        let result = self.resolve(&request);

        self.state.pending.write().insert(
            id.clone(),
            PendingRequest {
                request,
                result: Some(result),
            },
        );

        id
    }

    /// Poll for request result
    pub fn poll_data(&self, request_id: &str) -> DataResult {
        if let Some(pending) = self.state.pending.write().get_mut(request_id) {
            if let Some(result) = pending.result.take() {
                return result;
            }
        }

        DataResult::failed("Unknown request ID")
    }

    /// Check if data exists in any registered cache
    pub fn has_data(&self, key: &str) -> bool {
        let resolvers = self.resolvers.read();

        // Check cache sources only
        for source in [
            DataSource::MetadataStore,
            DataSource::ContentCache,
            DataSource::Memory,
        ] {
            if let Some(resolver) = resolvers.get(&source) {
                let dummy_request = DataRequest::new(key);
                if resolver.has(key, &dummy_request) {
                    return true;
                }
            }
        }

        false
    }

    /// Get data directly (sync)
    pub fn get_data(&self, key: &str) -> Option<Vec<u8>> {
        let request = DataRequest::cache_only(key);
        let result = self.resolve(&request);

        if result.status == DataStatus::Ready || result.status == DataStatus::Cached {
            result.data
        } else {
            None
        }
    }
}

impl Default for DataService {
    fn default() -> Self {
        Self::new()
    }
}
