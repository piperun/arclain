//! Data API Service
//!
//! Provides the unified data access layer using the resolver pattern.
//! Plugins and UI request data, the service resolves it from the appropriate source.

use super::types::{DataRequest, DataResult, DataSource, DataStatus, SourceChain};
use crate::features::resolver::DataSourceResolver;
use crate::shared::safe_log_fingerprint;
use crate::DEFAULT_MAX_RESOURCE_SIZE_BYTES;
use indexmap::{IndexMap, IndexSet};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::debug;

/// The legacy request/poll API resolves synchronously, so entries normally
/// live only until the SDK's next poll. Bound abandoned IDs so a plugin that
/// never polls cannot grow host memory without limit.
const MAX_PENDING_REQUESTS: usize = 256;
/// Aggregate body budget for results waiting to cross the request/poll ABI.
/// Large resources should use the cache-backed streaming API instead.
const MAX_PENDING_BODY_BYTES: usize = DEFAULT_MAX_RESOURCE_SIZE_BYTES;
/// Per-result diagnostic budget. Request keys and lower-layer diagnostics may
/// contain plugin-controlled text, so abandoned failures must not retain an
/// arbitrarily large string.
const MAX_PENDING_ERROR_BYTES: usize = 4 * 1024;

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
    /// Ceiling applied by every resolver before a body can cross this API.
    materialization_limit: Arc<AtomicUsize>,
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
    result: Option<DataResult>,
}

#[derive(Clone, Copy)]
struct PendingLimits {
    max_requests: usize,
    max_body_bytes: usize,
    max_error_bytes: usize,
}

const DEFAULT_PENDING_LIMITS: PendingLimits = PendingLimits {
    max_requests: MAX_PENDING_REQUESTS,
    max_body_bytes: MAX_PENDING_BODY_BYTES,
    max_error_bytes: MAX_PENDING_ERROR_BYTES,
};

impl PendingRequest {
    fn body_bytes(&self) -> usize {
        self.result
            .as_ref()
            .and_then(|result| result.data.as_ref())
            .map_or(0, Vec::len)
    }
}

impl PendingRequests {
    fn insert_with_limits(&mut self, id: String, mut result: DataResult, limits: PendingLimits) {
        if limits.max_requests == 0 {
            return;
        }

        if result
            .data
            .as_ref()
            .is_some_and(|body| body.len() > limits.max_body_bytes)
        {
            result = DataResult::failed(format!(
                "Data response exceeds pending body budget of {} bytes",
                limits.max_body_bytes
            ));
        }
        if let Some(error) = result.error.as_mut() {
            truncate_utf8_with_ellipsis(error, limits.max_error_bytes);
        }

        let request = PendingRequest {
            result: Some(result),
        };
        let body_bytes = request.body_bytes();

        while !self.entries.is_empty()
            && (self.entries.len() >= limits.max_requests
                || self.retained_body_bytes.saturating_add(body_bytes) > limits.max_body_bytes)
        {
            let (evicted_id, evicted) = self
                .entries
                .shift_remove_index(0)
                .expect("pending entries checked as non-empty");
            self.retained_body_bytes = self
                .retained_body_bytes
                .saturating_sub(evicted.body_bytes());
            debug!(
                "Evicted abandoned data request '{}'",
                safe_log_fingerprint(&evicted_id)
            );
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

fn truncate_utf8_with_ellipsis(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes && value.capacity() <= max_bytes {
        return;
    }

    const ELLIPSIS: &str = "…";
    let truncated = value.len() > max_bytes;
    let suffix = if truncated && max_bytes >= ELLIPSIS.len() {
        ELLIPSIS
    } else {
        ""
    };
    let mut end = if truncated {
        max_bytes.saturating_sub(suffix.len()).min(value.len())
    } else {
        value.len()
    };
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = String::with_capacity(end + suffix.len());
    bounded.push_str(&value[..end]);
    bounded.push_str(suffix);
    *value = bounded;
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
            materialization_limit: Arc::new(AtomicUsize::new(DEFAULT_MAX_RESOURCE_SIZE_BYTES)),
        }
    }

    /// Set the service ID (builder pattern)
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Set the maximum body this service may materialize or retain.
    pub fn set_materialization_limit(&self, limit: usize) {
        self.materialization_limit.store(limit, Ordering::Relaxed);
    }

    /// Return the current materialized-body ceiling.
    pub fn materialization_limit(&self) -> usize {
        self.materialization_limit.load(Ordering::Relaxed)
    }

    /// Register a resolver for a data source
    pub fn register_resolver(&self, source: DataSource, resolver: Arc<dyn DataSourceResolver>) {
        debug!(
            "[DataService:{}] Registered resolver for {:?}",
            safe_log_fingerprint(&self.id),
            source
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
            safe_log_fingerprint(&request.key),
            sources
        );

        let resolvers = self.snapshot_resolvers(sources);
        let store_resolvers = request
            .store_sources
            .as_ref()
            .map(|sources| self.snapshot_resolvers(sources))
            .unwrap_or_else(|| resolvers.clone());

        // Track real (non-NotFound) failures so they can be surfaced to
        // callers. Cache misses are uninteresting, but I/O errors from
        // the network resolver carry the actual diagnostic (HTTP 403,
        // timeout, DNS failure, etc.) and were previously swallowed by
        // the generic "No source could provide data" message.
        let mut io_errors: Vec<(DataSource, String)> = Vec::new();

        let materialization_limit = self.materialization_limit();
        for (source, resolver) in &resolvers {
            match resolver.try_resolve_with_limit(&request.key, request, materialization_limit) {
                Ok(data) => {
                    debug!(
                        "[DataService] Resolved '{}' from {:?} ({} bytes)",
                        safe_log_fingerprint(&request.key),
                        source,
                        data.len()
                    );

                    // If data came from network, try to store to earlier sources
                    if *source == DataSource::Network {
                        self.store_to_caches(&request.key, &data, request, &store_resolvers);
                    }

                    return DataResult::ready(data);
                }
                Err(e) => {
                    debug!(
                        "[DataService] Source {:?} failed for '{}': {}",
                        source,
                        safe_log_fingerprint(&request.key),
                        safe_log_fingerprint(e.to_string())
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
            debug!(
                "[DataService] Skipping cache for search key: {}",
                safe_log_fingerprint(key)
            );
            return;
        }

        for (source, resolver) in resolvers {
            // Only store to cache sources, not network
            if *source == DataSource::Network || !request.allows_store_to(*source) {
                continue;
            }

            if let Err(e) = resolver.try_store(key, data, request) {
                debug!(
                    "[DataService] Failed to store '{}' to {:?}: {}",
                    safe_log_fingerprint(key),
                    source,
                    safe_log_fingerprint(e.to_string())
                );
            } else {
                debug!(
                    "[DataService] Stored '{}' to {:?}",
                    safe_log_fingerprint(key),
                    source
                );
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

        self.state.pending.write().insert_with_limits(
            id.clone(),
            result,
            PendingLimits {
                max_body_bytes: self.materialization_limit(),
                ..DEFAULT_PENDING_LIMITS
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
        self.has_data_from_sources(
            key,
            [
                DataSource::MetadataStore,
                DataSource::ContentCache,
                DataSource::Memory,
            ],
        )
    }

    /// Check only the caller-approved sources for a key.
    pub fn has_data_from_sources(
        &self,
        key: &str,
        sources: impl IntoIterator<Item = DataSource>,
    ) -> bool {
        let request = DataRequest::new(key).with_sources(sources);
        self.has_data_for_request(&request)
    }

    /// Check the request's exact source set while preserving its owner
    /// context (notably `plugin_id`) for resolver isolation.
    pub fn has_data_for_request(&self, request: &DataRequest) -> bool {
        let sources = &request.sources;
        if sources.is_empty() {
            return false;
        }
        let resolvers = self.snapshot_resolvers(sources);

        // Check cache sources only
        for (_, resolver) in resolvers {
            if resolver.has_with_limit(&request.key, request, self.materialization_limit()) {
                return true;
            }
        }

        false
    }

    /// Get data directly (sync)
    pub fn get_data(&self, key: &str) -> Option<Vec<u8>> {
        self.get_data_from_sources(key, [DataSource::MetadataStore, DataSource::ContentCache])
    }

    /// Resolve a key only through the caller-approved source set.
    pub fn get_data_from_sources(
        &self,
        key: &str,
        sources: impl IntoIterator<Item = DataSource>,
    ) -> Option<Vec<u8>> {
        let sources: SourceChain = sources.into_iter().collect();
        let request = DataRequest::new(key)
            .with_sources(sources)
            .with_store_sources([]);
        self.get_data_for_request(&request)
    }

    /// Resolve the request's exact source set while preserving its owner
    /// context and prohibiting write-back.
    pub fn get_data_for_request(&self, request: &DataRequest) -> Option<Vec<u8>> {
        if request.sources.is_empty() {
            return None;
        }
        let mut request = request.clone();
        request.store_sources = Some(SourceChain::new());
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
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
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

    struct MetadataReadStoreSpy {
        body: Option<Vec<u8>>,
        store_calls: Arc<AtomicUsize>,
    }

    struct LimitAwareHasSpy {
        observed_limit: Arc<AtomicUsize>,
    }

    impl DataSourceResolver for LimitAwareHasSpy {
        fn try_resolve(&self, _key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
            panic!("limit-aware existence check must not materialize data")
        }

        fn has_with_limit(&self, _key: &str, _request: &DataRequest, limit: usize) -> bool {
            self.observed_limit.store(limit, AtomicOrdering::SeqCst);
            true
        }
    }

    impl DataSourceResolver for MetadataReadStoreSpy {
        fn try_resolve(&self, _key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
            self.body.clone().ok_or(ResolveError::NotFound)
        }

        fn try_store(
            &self,
            _key: &str,
            _data: &[u8],
            _request: &DataRequest,
        ) -> Result<(), ResolveError> {
            self.store_calls.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(())
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
    fn body_larger_than_materialization_budget_is_not_retained() {
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
            .is_some_and(|error| error.contains("materialized read limit")));
    }

    #[test]
    fn pending_failure_errors_are_utf8_safely_bounded() {
        const ERROR_LIMIT: usize = 17;
        let mut pending = PendingRequests::default();
        pending.insert_with_limits(
            "failure".to_string(),
            DataResult::failed("秘密".repeat(32)),
            PendingLimits {
                max_requests: 2,
                max_body_bytes: 128,
                max_error_bytes: ERROR_LIMIT,
            },
        );

        let result = pending.poll("failure").expect("poll bounded failure");
        let error = result.error.expect("failure should retain an error");
        assert!(error.len() <= ERROR_LIMIT);
        assert!(
            error.capacity() <= ERROR_LIMIT,
            "truncation must release the attacker-controlled backing allocation"
        );
        assert!(error.ends_with('…'));
        assert!(pending.entries.is_empty());
    }

    #[test]
    fn pending_failure_rebuilds_short_errors_with_oversized_capacity() {
        const ERROR_LIMIT: usize = 17;
        let mut attacker_error = String::with_capacity(1024 * 1024);
        attacker_error.push_str("秘密");
        assert!(attacker_error.capacity() > ERROR_LIMIT);

        let mut pending = PendingRequests::default();
        pending.insert_with_limits(
            "failure".to_string(),
            DataResult::failed(attacker_error),
            PendingLimits {
                max_requests: 2,
                max_body_bytes: 128,
                max_error_bytes: ERROR_LIMIT,
            },
        );

        let error = pending
            .poll("failure")
            .expect("poll bounded failure")
            .error
            .expect("failure should retain an error");
        assert_eq!(error, "秘密");
        assert!(error.capacity() <= ERROR_LIMIT);
    }

    #[test]
    fn configured_materialization_limit_rejects_resolver_results_and_bounds_pending_state() {
        const LIMIT: usize = 8;
        let service = service_with_body_size(LIMIT + 1);
        service.set_materialization_limit(LIMIT);

        let request_id = service.request_data(network_request("custom-limit"));
        let pending = service.state.pending.read();
        assert_eq!(pending.retained_body_bytes, 0);
        drop(pending);

        let result = service.poll_data(&request_id);
        assert_eq!(result.status, DataStatus::Failed);
        assert!(result.data.is_none());
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("8-byte materialized read limit")));
    }

    #[test]
    fn read_allowed_write_denied_request_can_resolve_metadata() {
        let service = DataService::new();
        let store_calls = Arc::new(AtomicUsize::new(0));
        service.register_resolver(
            DataSource::MetadataStore,
            Arc::new(MetadataReadStoreSpy {
                body: Some(b"metadata".to_vec()),
                store_calls: store_calls.clone(),
            }),
        );
        let request = DataRequest::new("metadata")
            .with_sources([DataSource::MetadataStore])
            .with_store_sources([]);

        let result = service.resolve(&request);

        assert_eq!(result.data.as_deref(), Some(b"metadata".as_slice()));
        assert_eq!(store_calls.load(AtomicOrdering::SeqCst), 0);
    }

    #[test]
    fn network_success_does_not_write_to_a_disallowed_metadata_store() {
        let service = DataService::new();
        let store_calls = Arc::new(AtomicUsize::new(0));
        service.register_resolver(
            DataSource::MetadataStore,
            Arc::new(MetadataReadStoreSpy {
                body: None,
                store_calls: store_calls.clone(),
            }),
        );
        service.register_resolver(
            DataSource::Network,
            Arc::new(FixedBodyResolver { body_bytes: 8 }),
        );
        let request = DataRequest::new("network")
            .with_sources([DataSource::MetadataStore, DataSource::Network])
            .with_store_sources([]);

        let result = service.resolve(&request);

        assert_eq!(result.status, DataStatus::Ready);
        assert_eq!(store_calls.load(AtomicOrdering::SeqCst), 0);
    }

    #[test]
    fn unrestricted_requests_preserve_legacy_network_write_back() {
        let service = DataService::new();
        let store_calls = Arc::new(AtomicUsize::new(0));
        service.register_resolver(
            DataSource::MetadataStore,
            Arc::new(MetadataReadStoreSpy {
                body: None,
                store_calls: store_calls.clone(),
            }),
        );
        service.register_resolver(
            DataSource::Network,
            Arc::new(FixedBodyResolver { body_bytes: 8 }),
        );
        let request = DataRequest::new("network")
            .with_sources([DataSource::MetadataStore, DataSource::Network]);

        assert_eq!(service.resolve(&request).status, DataStatus::Ready);
        assert_eq!(store_calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[test]
    fn explicit_store_sources_are_snapshotted_independently_from_read_sources() {
        let service = DataService::new();
        let store_calls = Arc::new(AtomicUsize::new(0));
        service.register_resolver(
            DataSource::ContentCache,
            Arc::new(MetadataReadStoreSpy {
                body: None,
                store_calls: store_calls.clone(),
            }),
        );
        service.register_resolver(
            DataSource::Network,
            Arc::new(FixedBodyResolver { body_bytes: 8 }),
        );
        let request = DataRequest::new("network")
            .with_sources([DataSource::Network])
            .with_store_sources([DataSource::ContentCache]);

        let result = service.resolve(&request);

        assert_eq!(result.status, DataStatus::Ready);
        assert_eq!(store_calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[test]
    fn source_specific_get_and_has_do_not_fall_back_to_disallowed_sources() {
        let service = DataService::new();
        service.register_resolver(
            DataSource::MetadataStore,
            Arc::new(MetadataReadStoreSpy {
                body: Some(b"private metadata".to_vec()),
                store_calls: Arc::new(AtomicUsize::new(0)),
            }),
        );

        assert!(!service.has_data_from_sources("key", [DataSource::ContentCache]));
        assert_eq!(
            service.get_data_from_sources("key", [DataSource::ContentCache]),
            None
        );
        assert!(!service.has_data_from_sources("key", []));
        assert_eq!(service.get_data_from_sources("key", []), None);
    }

    #[test]
    fn source_specific_has_propagates_the_service_materialization_limit() {
        const LIMIT: usize = 37;
        let service = DataService::new();
        service.set_materialization_limit(LIMIT);
        let observed_limit = Arc::new(AtomicUsize::new(0));
        service.register_resolver(
            DataSource::MetadataStore,
            Arc::new(LimitAwareHasSpy {
                observed_limit: observed_limit.clone(),
            }),
        );

        assert!(service.has_data_from_sources("key", [DataSource::MetadataStore]));
        assert_eq!(observed_limit.load(AtomicOrdering::SeqCst), LIMIT);
    }
}
