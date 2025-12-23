//! MetadataStore resolver
//!
//! Resolves structured metadata from the persistent MetadataStore.
//! Checks:
//! - MetadataStore (metadata.sqlite)
//! - ContentCache (cacache) for raw data

use super::{DataSourceResolver, ResolveError};
use crate::features::api::DataRequest;
use crate::features::resource_manager::ResourceManager;
use arclain_db::{MetadataStore, ProductMetadata};
use std::sync::Arc;

/// Resolver for structured metadata stored in MetadataStore
pub struct MetadataStoreResolver {
    store: Option<Arc<MetadataStore>>,
    resource_manager: Option<Arc<ResourceManager>>,
}

impl MetadataStoreResolver {
    pub fn new(store: Arc<MetadataStore>) -> Self {
        Self {
            store: Some(store),
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

        // For JSON/Metadata
        if let Some(store) = &self.store {
            let id = Self::key_to_id(key);
            if let Ok(Some(meta)) = store.get(&id) {
                // Return the ProductMetadata struct serialized as JSON
                // Note: The previous implementation returned a custom JSON structure.
                // We should ideally return the serialized ProductMetadata directly now,
                // but if consumers expect the old format, we might need a transformation.
                // For now, let's return the ProductMetadata JSON directly as the user wants to move to it.
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
        tracing::info!("[MetadataStoreResolver] try_store called for key: {}", key);

        // Skip raw API responses - these are not ProductMetadata structs
        // They come from network fetches and have prefixes like dlsite:json: or dlsite:html:
        if key.starts_with("dlsite:json:") || key.starts_with("dlsite:html:") {
            tracing::debug!(
                "[MetadataStoreResolver] Skipping raw API data for key: {} (not ProductMetadata)",
                key
            );
            return Ok(());
        }

        let store = self.store.as_ref().ok_or_else(|| {
            tracing::error!("[MetadataStoreResolver] Store not configured");
            ResolveError::NotConfigured
        })?;

        let meta: ProductMetadata = serde_json::from_slice(data).map_err(|e| {
            tracing::error!(
                "[MetadataStoreResolver] Failed to parse metadata JSON: {}",
                e
            );
            ResolveError::IoError(format!("Invalid metadata JSON: {}", e))
        })?;

        tracing::info!(
            "[MetadataStoreResolver] Saving metadata id={} source={}",
            meta.id,
            meta.source
        );

        store.save(&meta).map_err(|e| {
            tracing::error!("[MetadataStoreResolver] Save failed: {}", e);
            ResolveError::IoError(e.to_string())
        })?;

        tracing::info!("[MetadataStoreResolver] Saved successfully: {}", meta.id);
        Ok(())
    }

    fn has(&self, key: &str, _request: &DataRequest) -> bool {
        let id = Self::key_to_id(key);

        if let Some(store) = &self.store {
            if let Ok(Some(_)) = store.get(&id) {
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
