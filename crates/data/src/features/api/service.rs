//! Data API Service
//!
//! Provides the unified data access layer using the resolver pattern.
//! Plugins and UI request data, the service resolves it from the appropriate source.

use super::types::{DataRequest, DataResult, DataSource, DataStatus, SourceChain};
use crate::features::resolver::DataSourceResolver;
use indexmap::{IndexMap, IndexSet};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

/// The legacy request/poll API resolves synchronously, so entries normally
/// live only until the SDK's next poll. Bound abandoned IDs so a plugin that
/// never polls cannot grow host memory without limit.
const MAX_PENDING_REQUESTS: usize = 256;
/// Aggregate body budget for results waiting to cross the request/poll ABI.
/// Large resources should use the cache-backed streaming API instead.
const MAX_PENDING_BODY_BYTES: usize = 50 * 1024 * 1024;

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
    pending: RwLock<PendingRequests>,
    next_id: RwLock<u64>,
}

#[derive(Default)]
struct PendingRequests {
    entries: IndexMap<String, PendingRequest>,
    retained_body_bytes: usize,
}

struct PendingRequest {
    #[allow(dead_code)] // Kept for debugging/logging
    request: DataRequest,
    result: Option<DataResult>,
}

impl PendingRequest {
    fn body_bytes(&self) -> usize {
        self.result
            .as_ref()
            .and_then(|result| result.data.as_ref())
            .map_or(0, Vec::len)
    }
}

impl PendingRequests {
    fn insert(&mut self, id: String, mut request: PendingRequest) {
        let mut body_bytes = request.body_bytes();
        if body_bytes > MAX_PENDING_BODY_BYTES {
            request.result = Some(DataResult::failed(format!(
                "Data response exceeds pending body budget of {MAX_PENDING_BODY_BYTES} bytes"
            )));
            body_bytes = 0;
        }

        while !self.entries.is_empty()
            && (self.entries.len() >= MAX_PENDING_REQUESTS
                || self.retained_body_bytes.saturating_add(body_bytes) > MAX_PENDING_BODY_BYTES)
        {
            let (evicted_id, evicted) = self
                .entries
                .shift_remove_index(0)
                .expect("pending entries checked as non-empty");
            self.retained_body_bytes = self
                .retained_body_bytes
                .saturating_sub(evicted.body_bytes());
            debug!("Evicted abandoned data request '{evicted_id}'");
        }

        self.retained_body_bytes = self.retained_body_bytes.saturating_add(body_bytes);
        self.entries.insert(id, request);
    }

    fn poll(&mut self, request_id: &str) -> Option<DataResult> {
        let is_terminal = self
            .entries
            .get(request_id)?
            .result
            .as_ref()
            .is_some_and(|result| {
                matches!(
                    result.status,
                    DataStatus::Ready | DataStatus::Failed | DataStatus::Cached
                )
            });

        if !is_terminal {
            return self.entries.get(request_id)?.result.clone();
        }

        let mut request = self.entries.shift_remove(request_id)?;
        self.retained_body_bytes = self
            .retained_body_bytes
            .saturating_sub(request.body_bytes());
        request.result.take()
    }
}

type ResolverSnapshot = Vec<(DataSource, Arc<dyn DataSourceResolver>)>;

impl DataApiState {
    fn new() -> Self {
        Self {
            pending: RwLock::new(PendingRequests::default()),
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
        debug!(
            "[DataService:{}] Registered resolver for {:?}",
            self.id, source
        );
        self.resolvers.write().insert(source, resolver);
    }

    fn snapshot_resolvers<'a>(
        &self,
        sources: impl IntoIterator<Item = &'a DataSource>,
    ) -> ResolverSnapshot {
        let resolvers = self.resolvers.read();
        sources
            .into_iter()
            .filter_map(|source| {
                resolvers
                    .get(source)
                    .cloned()
                    .map(|resolver| (*source, resolver))
            })
            .collect()
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
        let resolver = self.resolvers.read().get(&source).cloned();
        if let Some(resolver) = resolver {
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

        let resolvers = self.snapshot_resolvers(sources);

        // Track real (non-NotFound) failures so they can be surfaced to
        // callers. Cache misses are uninteresting, but I/O errors from
        // the network resolver carry the actual diagnostic (HTTP 403,
        // timeout, DNS failure, etc.) and were previously swallowed by
        // the generic "No source could provide data" message.
        let mut io_errors: Vec<(DataSource, String)> = Vec::new();

        for (source, resolver) in &resolvers {
            match resolver.try_resolve(&request.key, request) {
                Ok(data) => {
                    debug!(
                        "[DataService] Resolved '{}' from {:?} ({} bytes)",
                        request.key,
                        source,
                        data.len()
                    );

                    // If data came from network, try to store to earlier sources
                    if *source == DataSource::Network {
                        self.store_to_caches(&request.key, &data, request, &resolvers);
                    }

                    return DataResult::ready(data);
                }
                Err(e) => {
                    debug!(
                        "[DataService] Source {:?} failed for '{}': {}",
                        source, request.key, e
                    );
                    if let crate::features::resolver::ResolveError::IoError(msg) = &e {
                        io_errors.push((*source, msg.clone()));
                    }
                    continue;
                }
            }
        }

        // Prefer the first IoError as the user-facing message — it
        // explains *why* the fetch failed (HTTP status, network error)
        // instead of hiding behind a generic "no source" message.
        if let Some((source, msg)) = io_errors.into_iter().next() {
            return DataResult::failed(format!(
                "Failed to fetch '{}' ({:?}): {}",
                request.key, source, msg
            ));
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
        resolvers: &ResolverSnapshot,
    ) {
        // Don't cache search results - they're ephemeral and change over time
        if key.contains(":search:") {
            debug!("[DataService] Skipping cache for search key: {}", key);
            return;
        }

        for (source, resolver) in resolvers {
            // Only store to cache sources, not network
            if *source == DataSource::Network {
                continue;
            }

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
        if let Some(result) = self.state.pending.write().poll(request_id) {
            return result;
        }

        DataResult::failed("Unknown request ID")
    }

    /// Check if data exists in any registered cache
    pub fn has_data(&self, key: &str) -> bool {
        let sources = [
            DataSource::MetadataStore,
            DataSource::ContentCache,
            DataSource::Memory,
        ];
        let resolvers = self.snapshot_resolvers(&sources);

        // Check cache sources only
        for (_, resolver) in resolvers {
            let dummy_request = DataRequest::new(key);
            if resolver.has(key, &dummy_request) {
                return true;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::resolver::ResolveError;
    use std::sync::Barrier;
    use std::thread;

    const EXPECTED_MAX_PENDING_REQUESTS: usize = 256;
    const EXPECTED_MAX_PENDING_BODY_BYTES: usize = 50 * 1024 * 1024;
    const RESPONSE_BYTES: usize = 1024;

    struct FixedBodyResolver {
        body_bytes: usize,
    }

    impl DataSourceResolver for FixedBodyResolver {
        fn try_resolve(&self, _key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
            Ok(vec![0xA5; self.body_bytes])
        }
    }

    fn service_with_body_resolver() -> DataService {
        service_with_body_size(RESPONSE_BYTES)
    }

    fn service_with_body_size(body_bytes: usize) -> DataService {
        let service = DataService::new();
        service.register_resolver(
            DataSource::Network,
            Arc::new(FixedBodyResolver { body_bytes }),
        );
        service
    }

    fn network_request(key: impl Into<String>) -> DataRequest {
        DataRequest::new(key).with_sources([DataSource::Network])
    }

    #[test]
    fn terminal_poll_removes_the_whole_pending_request() {
        let service = service_with_body_resolver();
        let request_id = service.request_data(network_request("payload"));

        assert_eq!(service.state.pending.read().entries.len(), 1);
        let result = service.poll_data(&request_id);

        assert_eq!(result.status, DataStatus::Ready);
        assert_eq!(result.data.as_ref().map(Vec::len), Some(RESPONSE_BYTES));
        assert!(service.state.pending.read().entries.is_empty());
    }

    #[test]
    fn repeat_poll_after_terminal_result_is_unknown() {
        let service = service_with_body_resolver();
        let request_id = service.request_data(network_request("payload"));

        assert_eq!(service.poll_data(&request_id).status, DataStatus::Ready);
        let repeated = service.poll_data(&request_id);

        assert_eq!(repeated.status, DataStatus::Failed);
        assert_eq!(repeated.error.as_deref(), Some("Unknown request ID"));
        assert!(repeated.data.is_none());
    }

    #[test]
    fn concurrent_terminal_polls_have_one_consumer_and_leave_no_entry() {
        let service = service_with_body_resolver();
        let request_id = service.request_data(network_request("payload"));
        let start = Arc::new(Barrier::new(3));

        let workers: Vec<_> = (0..2)
            .map(|_| {
                let service = service.clone();
                let request_id = request_id.clone();
                let start = start.clone();
                thread::spawn(move || {
                    start.wait();
                    service.poll_data(&request_id)
                })
            })
            .collect();

        start.wait();
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().expect("poll worker panicked"))
            .collect();

        assert_eq!(
            results
                .iter()
                .filter(|result| result.status == DataStatus::Ready)
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| {
                    result.status == DataStatus::Failed
                        && result.error.as_deref() == Some("Unknown request ID")
                })
                .count(),
            1
        );
        assert!(service.state.pending.read().entries.is_empty());
    }

    #[test]
    fn abandoned_terminal_requests_are_bounded_and_evict_oldest_first() {
        let service = service_with_body_resolver();
        let mut request_ids = Vec::new();

        for index in 0..=EXPECTED_MAX_PENDING_REQUESTS {
            request_ids.push(service.request_data(network_request(format!("payload-{index}"))));
        }

        let pending = service.state.pending.read();
        assert_eq!(pending.entries.len(), EXPECTED_MAX_PENDING_REQUESTS);
        let retained_bytes: usize = pending
            .entries
            .values()
            .filter_map(|request| request.result.as_ref()?.data.as_ref())
            .map(Vec::len)
            .sum();
        assert_eq!(
            retained_bytes,
            EXPECTED_MAX_PENDING_REQUESTS * RESPONSE_BYTES
        );
        drop(pending);

        let evicted = service.poll_data(&request_ids[0]);
        assert_eq!(evicted.status, DataStatus::Failed);
        assert_eq!(evicted.error.as_deref(), Some("Unknown request ID"));
        assert_eq!(
            service
                .poll_data(request_ids.last().expect("latest request ID"))
                .status,
            DataStatus::Ready
        );
    }

    #[test]
    fn abandoned_terminal_body_bytes_evict_oldest_before_crossing_budget() {
        const BODY_BYTES: usize = 5 * 1024 * 1024;
        let service = service_with_body_size(BODY_BYTES);
        let mut request_ids = Vec::new();

        for index in 0..=EXPECTED_MAX_PENDING_BODY_BYTES / BODY_BYTES {
            request_ids.push(service.request_data(network_request(format!("body-{index}"))));
        }

        let pending = service.state.pending.read();
        assert_eq!(pending.retained_body_bytes, EXPECTED_MAX_PENDING_BODY_BYTES);
        assert_eq!(pending.entries.len(), 10);
        drop(pending);

        let evicted = service.poll_data(&request_ids[0]);
        assert_eq!(evicted.status, DataStatus::Failed);
        assert_eq!(evicted.error.as_deref(), Some("Unknown request ID"));
        assert_eq!(
            service
                .poll_data(request_ids.last().expect("latest request ID"))
                .status,
            DataStatus::Ready
        );
    }

    #[test]
    fn body_larger_than_aggregate_budget_is_not_retained() {
        let service = service_with_body_size(EXPECTED_MAX_PENDING_BODY_BYTES + 1);
        let request_id = service.request_data(network_request("oversized"));

        let pending = service.state.pending.read();
        assert_eq!(pending.retained_body_bytes, 0);
        assert_eq!(pending.entries.len(), 1);
        drop(pending);

        let result = service.poll_data(&request_id);
        assert_eq!(result.status, DataStatus::Failed);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("pending body budget")));
    }
}
