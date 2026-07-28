//! MetadataStore resolver
//!
//! Resolves structured metadata from a persistent metadata store.
//! Checks:
//! - `MetadataReader` (e.g. `arclain_core::LibraryService` →
//!   library.sqlite via Diesel)
//! - `ContentCache` (cacache) for raw data
//!
//! The `MetadataReader` indirection breaks the old hard dep on
//! `arclain_core::LibraryService` — see `crate::traits` and the audit
//! note about the data→core cycle.

use super::{default_materialization_limit, DataSourceResolver, ResolveError};
use crate::features::api::DataRequest;
use crate::features::resource_manager::ResourceManager;
use crate::shared::{safe_log_fingerprint, serialize_json_with_limit};
use crate::traits::MetadataReader;
use arclain_db::ProductMetadata;
use std::sync::Arc;

/// Resolver for structured metadata stored via a `MetadataReader`
pub struct MetadataStoreResolver {
    library_service: Option<Arc<dyn MetadataReader>>,
    resource_manager: Option<Arc<ResourceManager>>,
}

impl MetadataStoreResolver {
    pub fn new(library_service: Arc<dyn MetadataReader>) -> Self {
        Self {
            library_service: Some(library_service),
            resource_manager: None,
        }
    }

    pub fn with_resource_manager(mut self, manager: Arc<ResourceManager>) -> Self {
        self.resource_manager = Some(manager);
        self
    }

    pub fn set_resource_manager(&mut self, manager: Arc<ResourceManager>) {
        self.resource_manager = Some(manager);
    }

    // Helper to format key to ID
    fn key_to_id(key: &str) -> String {
        let product_id = key
            .strip_prefix("dlsite:json:")
            .or_else(|| key.strip_prefix("dlsite:html:"))
            .unwrap_or(key);

        // Assuming DLSite for now effectively, but we should probably handle sources better.
        // For now, if it doesn't have a source prefix in the ID, assume dlsite.
        if product_id.contains(':') {
            product_id.to_string()
        } else {
            format!("dlsite:{}", product_id)
        }
    }
}

impl DataSourceResolver for MetadataStoreResolver {
    fn try_resolve(&self, key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
        self.try_resolve_with_limit(key, _request, default_materialization_limit())
    }

    fn try_resolve_with_limit(
        &self,
        key: &str,
        _request: &DataRequest,
        limit: usize,
    ) -> Result<Vec<u8>, ResolveError> {
        let is_html_key = key.starts_with("dlsite:html:");

        // For HTML, go to ContentCache
        if is_html_key {
            if let Some(rm) = &self.resource_manager {
                if let Some(data) = rm.get_with_limit(key, limit) {
                    return Ok(data.data);
                }
            }
            return Err(ResolveError::NotFound);
        }

        // For JSON/Metadata - use LibraryService
        if let Some(lib_svc) = &self.library_service {
            let id = Self::key_to_id(key);
            if let Ok(Some(meta)) = lib_svc.get_metadata(&id) {
                // Return the ProductMetadata struct serialized as JSON
                let json = serialize_json_with_limit(&meta, limit, "metadata response")
                    .map_err(|error| ResolveError::IoError(error.to_string()))?;
                return Ok(json);
            }
        }

        // Fallback to ContentCache if not in Store (e.g. raw JSON files)
        if let Some(rm) = &self.resource_manager {
            if let Some(data) = rm.get_with_limit(key, limit) {
                return Ok(data.data);
            }
        }

        Err(ResolveError::NotFound)
    }

    fn try_store(
        &self,
        key: &str,
        data: &[u8],
        _request: &DataRequest,
    ) -> Result<(), ResolveError> {
        tracing::debug!(
            "[MetadataStoreResolver] try_store called for key: {}",
            safe_log_fingerprint(key)
        );

        // Skip raw API responses - these are not ProductMetadata structs
        // They come from network fetches and have prefixes like dlsite:json: or dlsite:html:
        if key.starts_with("dlsite:json:") || key.starts_with("dlsite:html:") {
            tracing::debug!(
                "[MetadataStoreResolver] Skipping raw API data for key: {} (not ProductMetadata)",
                safe_log_fingerprint(key)
            );
            return Ok(());
        }

        let lib_svc = self.library_service.as_ref().ok_or_else(|| {
            tracing::error!("[MetadataStoreResolver] LibraryService not configured");
            ResolveError::NotConfigured
        })?;

        let meta: ProductMetadata = serde_json::from_slice(data).map_err(|e| {
            tracing::error!(
                "[MetadataStoreResolver] Failed to parse metadata JSON: {}",
                safe_log_fingerprint(e.to_string())
            );
            ResolveError::IoError(format!("Invalid metadata JSON: {}", e))
        })?;

        tracing::debug!(
            "[MetadataStoreResolver] Saving metadata id={} source={}",
            safe_log_fingerprint(&meta.id),
            safe_log_fingerprint(meta.source.as_str())
        );

        lib_svc.save_metadata(&meta).map_err(|e| {
            tracing::error!(
                "[MetadataStoreResolver] Save failed: {}",
                safe_log_fingerprint(e.to_string())
            );
            ResolveError::IoError(e.to_string())
        })?;

        tracing::debug!(
            "[MetadataStoreResolver] Saved successfully: {}",
            safe_log_fingerprint(&meta.id)
        );
        Ok(())
    }

    fn has(&self, key: &str, _request: &DataRequest) -> bool {
        self.has_with_limit(key, _request, default_materialization_limit())
    }

    fn has_with_limit(&self, key: &str, _request: &DataRequest, _limit: usize) -> bool {
        let id = Self::key_to_id(key);

        if !key.starts_with("dlsite:html:") {
            if let Some(lib_svc) = &self.library_service {
                if lib_svc.has_metadata(&id).unwrap_or(false) {
                    return true;
                }
            }
        }

        if let Some(rm) = &self.resource_manager {
            if rm.has(key) {
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use arclain_db::{MetadataSource, ProductMetadata};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FixedMetadataReader {
        metadata: ProductMetadata,
    }

    impl MetadataReader for FixedMetadataReader {
        fn get_metadata(&self, _id: &str) -> Result<Option<ProductMetadata>> {
            Ok(Some(self.metadata.clone()))
        }

        fn has_metadata(&self, _id: &str) -> Result<bool> {
            Ok(true)
        }

        fn save_metadata(&self, _meta: &ProductMetadata) -> Result<()> {
            Ok(())
        }
    }

    fn resolver_with_large_metadata() -> MetadataStoreResolver {
        let mut metadata = ProductMetadata::new(MetadataSource::DLSite, "RJ000001");
        metadata.description = Some("x".repeat(256));
        MetadataStoreResolver::new(Arc::new(FixedMetadataReader { metadata }))
    }

    #[test]
    fn metadata_serialization_stops_at_the_caller_limit() {
        let resolver = resolver_with_large_metadata();
        let request = DataRequest::new("dlsite:RJ000001");

        let error = resolver
            .try_resolve_with_limit("dlsite:RJ000001", &request, 64)
            .expect_err("oversized metadata must fail during bounded serialization");

        assert!(error
            .to_string()
            .contains("64-byte materialized read limit"));
    }

    #[test]
    fn metadata_serialization_accepts_its_exact_boundary() {
        let resolver = resolver_with_large_metadata();
        let request = DataRequest::new("dlsite:RJ000001");
        let expected = resolver
            .try_resolve("dlsite:RJ000001", &request)
            .expect("default limit should accept test metadata");

        let actual = resolver
            .try_resolve_with_limit("dlsite:RJ000001", &request, expected.len())
            .expect("exact serialization boundary should be accepted");

        assert_eq!(actual, expected);
    }

    struct ExistenceOnlyMetadataReader {
        get_calls: AtomicUsize,
        has_calls: AtomicUsize,
    }

    impl MetadataReader for ExistenceOnlyMetadataReader {
        fn get_metadata(&self, _id: &str) -> Result<Option<ProductMetadata>> {
            self.get_calls.fetch_add(1, Ordering::SeqCst);
            panic!("metadata existence must not load the row")
        }

        fn has_metadata(&self, id: &str) -> Result<bool> {
            self.has_calls.fetch_add(1, Ordering::SeqCst);
            Ok(id == "dlsite:RJ000001")
        }

        fn save_metadata(&self, _meta: &ProductMetadata) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn metadata_has_uses_the_cheap_existence_query_without_loading_the_row() {
        let reader = Arc::new(ExistenceOnlyMetadataReader {
            get_calls: AtomicUsize::new(0),
            has_calls: AtomicUsize::new(0),
        });
        let resolver = MetadataStoreResolver::new(reader.clone());
        let request = DataRequest::new("dlsite:RJ000001");

        assert!(resolver.has_with_limit("dlsite:RJ000001", &request, 1));
        assert_eq!(reader.has_calls.load(Ordering::SeqCst), 1);
        assert_eq!(reader.get_calls.load(Ordering::SeqCst), 0);
    }
}
