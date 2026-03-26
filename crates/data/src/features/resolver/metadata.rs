//! MetadataStore resolver
//!
//! Resolves structured metadata from the persistent LibraryService.
//! Checks:
//! - LibraryService (library.sqlite via Diesel)
//! - ContentCache (cacache) for raw data

use super::{DataSourceResolver, ResolveError};
use crate::features::api::DataRequest;
use crate::features::resource_manager::ResourceManager;
use arclain_core::{LibraryService, ProductMetadata};
use std::sync::Arc;

/// Resolver for structured metadata stored via LibraryService
pub struct MetadataStoreResolver {
    library_service: Option<Arc<LibraryService>>,
    resource_manager: Option<Arc<ResourceManager>>,
}

impl MetadataStoreResolver {
    pub fn new(library_service: Arc<LibraryService>) -> Self {
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
        let is_html_key = key.starts_with("dlsite:html:");

        // For HTML, go to ContentCache
        if is_html_key {
            if let Some(rm) = &self.resource_manager {
                if let Some(data) = rm.get(key) {
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
                let json =
                    serde_json::to_vec(&meta).map_err(|e| ResolveError::IoError(e.to_string()))?;
                return Ok(json);
            }
        }

        // Fallback to ContentCache if not in Store (e.g. raw JSON files)
        if let Some(rm) = &self.resource_manager {
            if let Some(data) = rm.get(key) {
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
        tracing::debug!("[MetadataStoreResolver] try_store called for key: {}", key);

        // Skip raw API responses - these are not ProductMetadata structs
        // They come from network fetches and have prefixes like dlsite:json: or dlsite:html:
        if key.starts_with("dlsite:json:") || key.starts_with("dlsite:html:") {
            tracing::debug!(
                "[MetadataStoreResolver] Skipping raw API data for key: {} (not ProductMetadata)",
                key
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
                e
            );
            ResolveError::IoError(format!("Invalid metadata JSON: {}", e))
        })?;

        tracing::debug!(
            "[MetadataStoreResolver] Saving metadata id={} source={}",
            meta.id,
            meta.source.as_str()
        );

        lib_svc.save_metadata(&meta).map_err(|e| {
            tracing::error!("[MetadataStoreResolver] Save failed: {}", e);
            ResolveError::IoError(e.to_string())
        })?;

        tracing::debug!("[MetadataStoreResolver] Saved successfully: {}", meta.id);
        Ok(())
    }

    fn has(&self, key: &str, _request: &DataRequest) -> bool {
        let id = Self::key_to_id(key);

        if let Some(lib_svc) = &self.library_service {
            if let Ok(Some(_)) = lib_svc.get_metadata(&id) {
                return true;
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
