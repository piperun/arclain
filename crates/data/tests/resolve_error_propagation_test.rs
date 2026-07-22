//! Regression test: when a resolver returns a useful error (e.g.
//! Network IoError "HTTP error: 403"), it must be surfaced to callers.
//!
//! Previously, when all sources failed, `DataService::resolve()`
//! discarded each resolver's specific failure and returned a generic
//! "No source could provide data for '<key>'" — hiding the actual
//! HTTP status, timeout, or DNS error from the caller.

use arclain_data::{
    DataRequest, DataService, DataSource, DataSourceResolver, DataStatus, NetworkResolver,
    ResolveError,
};
use arclain_network::AsyncHttpClient;
use std::sync::Arc;

/// Test resolver that always returns NotFound (cache miss)
struct AlwaysNotFound;

impl DataSourceResolver for AlwaysNotFound {
    fn try_resolve(&self, _key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
        Err(ResolveError::NotFound)
    }
}

/// Test resolver that always returns IoError with a specific message
struct AlwaysIoError(&'static str);

impl DataSourceResolver for AlwaysIoError {
    fn try_resolve(&self, _key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
        Err(ResolveError::IoError(self.0.to_string()))
    }
}

/// When all sources return errors, the resolver-level errors should
/// be surfaced rather than just "No source could provide data".
#[test]
fn network_ioerror_is_surfaced_to_caller() {
    let svc = DataService::new();
    svc.register_resolver(DataSource::MetadataStore, Arc::new(AlwaysNotFound));
    svc.register_resolver(DataSource::ContentCache, Arc::new(AlwaysNotFound));
    svc.register_resolver(
        DataSource::Network,
        Arc::new(AlwaysIoError("HTTP error: 403 Forbidden")),
    );

    let mut sources = arclain_data::IndexSet::new();
    sources.insert(DataSource::MetadataStore);
    sources.insert(DataSource::ContentCache);
    sources.insert(DataSource::Network);

    let req = DataRequest::new("some-source:search:test-key")
        .with_url("https://example.com/search/test")
        .with_sources(sources);

    let result = svc.resolve(&req);

    assert_eq!(result.status, DataStatus::Failed);
    let err = result.error.expect("should have error");

    assert!(
        err.contains("403") || err.contains("Forbidden"),
        "Expected the underlying network error '403 Forbidden' to be \
         surfaced in the failure message, got: {}",
        err
    );
}

/// When only cache misses occurred (no network configured), keep the
/// generic "No source could provide data" message — there's nothing
/// useful to surface.
#[test]
fn pure_cache_miss_keeps_generic_message() {
    let svc = DataService::new();
    svc.register_resolver(DataSource::MetadataStore, Arc::new(AlwaysNotFound));
    svc.register_resolver(DataSource::ContentCache, Arc::new(AlwaysNotFound));

    let mut sources = arclain_data::IndexSet::new();
    sources.insert(DataSource::MetadataStore);
    sources.insert(DataSource::ContentCache);

    let req = DataRequest::new("some-key").with_sources(sources);

    let result = svc.resolve(&req);

    assert_eq!(result.status, DataStatus::Failed);
    let err = result.error.expect("should have error");
    assert!(
        err.contains("No source could provide data") || err.contains("not found"),
        "expected generic message for pure cache miss, got: {}",
        err
    );
}

/// IoError takes precedence over NotConfigured when both are present.
#[test]
fn ioerror_preferred_over_notconfigured() {
    struct AlwaysNotConfigured;
    impl DataSourceResolver for AlwaysNotConfigured {
        fn try_resolve(&self, _key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
            Err(ResolveError::NotConfigured)
        }
    }

    let svc = DataService::new();
    svc.register_resolver(DataSource::MetadataStore, Arc::new(AlwaysNotConfigured));
    svc.register_resolver(
        DataSource::Network,
        Arc::new(AlwaysIoError("connection timed out")),
    );

    let mut sources = arclain_data::IndexSet::new();
    sources.insert(DataSource::MetadataStore);
    sources.insert(DataSource::Network);

    let req = DataRequest::new("k")
        .with_url("https://example.com")
        .with_sources(sources);

    let result = svc.resolve(&req);
    let err = result.error.expect("should have error");

    assert!(
        err.contains("timed out") || err.contains("timeout"),
        "expected I/O error message to win over NotConfigured, got: {}",
        err
    );
}

#[test]
fn plugin_network_resolver_exposes_a_bound_plugin_constructor() {
    fn construct(client: Arc<AsyncHttpClient>) -> NetworkResolver {
        NetworkResolver::for_plugin(client, "bound-plugin")
    }

    let _current_signature: fn(Arc<AsyncHttpClient>) -> NetworkResolver = construct;
}
