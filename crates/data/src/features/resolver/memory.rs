//! Memory resolver
//!
//! In-memory ephemeral storage for temporary data.

use super::{DataSourceResolver, ResolveError};
use crate::features::api::DataRequest;
use parking_lot::RwLock;
use std::collections::HashMap;

/// Resolver for in-memory ephemeral data
pub struct MemoryResolver {
    store: RwLock<HashMap<String, Vec<u8>>>,
}

impl MemoryResolver {
    pub fn new() -> Self {
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
        self.store
            .read()
            .get(key)
            .cloned()
            .ok_or(ResolveError::NotFound)
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
}
