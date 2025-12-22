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
