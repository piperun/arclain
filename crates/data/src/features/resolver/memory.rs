//! Memory resolver
//!
//! In-memory ephemeral storage for temporary data.
//!
//! This resolver is intentionally not re-exported from `arclain_data`
//! — it's only exercised by the tests in this module, which validate
//! that the `DataSourceResolver` trait works correctly with a minimal
//! in-memory backing store. Production code uses the disk- and
//! cache-backed resolvers instead.

use super::{
    default_materialization_limit, materialized_limit_error, DataSourceResolver, ResolveError,
};
use crate::features::api::DataRequest;
use parking_lot::RwLock;
use std::collections::HashMap;

/// Resolver for in-memory ephemeral data. Test-only — see module docs.
#[allow(dead_code)]
pub(crate) struct MemoryResolver {
    store: RwLock<HashMap<String, Vec<u8>>>,
}

#[allow(dead_code)]
impl MemoryResolver {
    pub(crate) fn new() -> Self {
        Self {
            store: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemoryResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl DataSourceResolver for MemoryResolver {
    fn try_resolve(&self, key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
        self.try_resolve_with_limit(key, _request, default_materialization_limit())
    }

    fn try_resolve_with_limit(
        &self,
        key: &str,
        _request: &DataRequest,
        limit: usize,
    ) -> Result<Vec<u8>, ResolveError> {
        let store = self.store.read();
        let data = store.get(key).ok_or(ResolveError::NotFound)?;
        if data.len() > limit {
            return Err(materialized_limit_error(limit));
        }
        Ok(data.clone())
    }

    fn try_store(
        &self,
        key: &str,
        data: &[u8],
        _request: &DataRequest,
    ) -> Result<(), ResolveError> {
        self.store.write().insert(key.to_string(), data.to_vec());
        Ok(())
    }

    fn has(&self, key: &str, _request: &DataRequest) -> bool {
        self.store.read().contains_key(key)
    }

    fn has_with_limit(&self, key: &str, request: &DataRequest, _limit: usize) -> bool {
        self.has(key, request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_request() -> DataRequest {
        DataRequest::new("dummy")
    }

    #[test]
    fn store_and_resolve() {
        let resolver = MemoryResolver::new();
        let req = dummy_request();

        resolver.try_store("key1", b"hello", &req).unwrap();
        let data = resolver.try_resolve("key1", &req).unwrap();
        assert_eq!(data, b"hello");
    }

    #[test]
    fn resolve_missing_key() {
        let resolver = MemoryResolver::new();
        let req = dummy_request();

        let result = resolver.try_resolve("nonexistent", &req);
        assert!(result.is_err());
    }

    #[test]
    fn has_returns_correct_state() {
        let resolver = MemoryResolver::new();
        let req = dummy_request();

        assert!(!resolver.has("k", &req));
        resolver.try_store("k", b"data", &req).unwrap();
        assert!(resolver.has("k", &req));
    }

    #[test]
    fn multiple_entries_coexist() {
        let resolver = MemoryResolver::new();
        let req = dummy_request();

        resolver.try_store("a", b"alpha", &req).unwrap();
        resolver.try_store("b", b"beta", &req).unwrap();

        assert_eq!(resolver.try_resolve("a", &req).unwrap(), b"alpha");
        assert_eq!(resolver.try_resolve("b", &req).unwrap(), b"beta");
    }

    #[test]
    fn overwrite_existing_key() {
        let resolver = MemoryResolver::new();
        let req = dummy_request();

        resolver.try_store("k", b"old", &req).unwrap();
        resolver.try_store("k", b"new", &req).unwrap();
        assert_eq!(resolver.try_resolve("k", &req).unwrap(), b"new");
    }

    #[test]
    fn bounded_resolve_checks_length_before_cloning() {
        let resolver = MemoryResolver::new();
        let req = dummy_request();
        resolver.try_store("large", b"123456789", &req).unwrap();

        let error = resolver
            .try_resolve_with_limit("large", &req, 8)
            .expect_err("oversized memory entry must be rejected");

        assert!(error.to_string().contains("8-byte materialized read limit"));
    }
}
