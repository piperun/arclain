//! MetadataCache resolver
//!
//! Resolves structured metadata from SQLite cache.
//! Checks both:
//! - New unified product_metadata table (ProductMetadata)
//! - Legacy dlsite_metadata_cache table (MetadataCache)
//! - ContentCache (cacache) for raw data

use super::{DataSourceResolver, ResolveError};
use crate::features::api::DataRequest;
use crate::features::resource_manager::ResourceManager;
use arclain_db::{MetadataCache, MetadataSource, SqliteDb};
use std::sync::Arc;

/// Resolver for structured metadata stored in SQLite
/// Checks new ProductMetadata table, falls back to legacy MetadataCache, then ContentCache
pub struct MetadataCacheResolver {
    cache: Arc<MetadataCache>,
    cache_db: Option<Arc<SqliteDb>>,
    resource_manager: Option<Arc<ResourceManager>>,
}

impl MetadataCacheResolver {
    pub fn new(cache: Arc<MetadataCache>) -> Self {
        Self {
            cache,
            cache_db: None,
            resource_manager: None,
        }
    }

    pub fn with_resource_manager(mut self, manager: Arc<ResourceManager>) -> Self {
        self.resource_manager = Some(manager);
        self
    }

    pub fn with_cache_db(mut self, db: Arc<SqliteDb>) -> Self {
        self.cache_db = Some(db);
        self
    }

    pub fn set_resource_manager(&mut self, manager: Arc<ResourceManager>) {
        self.resource_manager = Some(manager);
    }

    pub fn set_cache_db(&mut self, db: Arc<SqliteDb>) {
        self.cache_db = Some(db);
    }

    /// Try to get metadata from the new ProductMetadata table
    fn try_new_table(&self, product_id: &str) -> Option<Vec<u8>> {
        let cache_db = self.cache_db.as_ref()?;

        // Try to load from new product_metadata table
        let result = cache_db.with_connection(|conn| {
            arclain_db::get_by_external_id(conn, MetadataSource::DLSite, product_id)
        });

        match result {
            Ok(Some(meta)) => {
                tracing::info!(
                    "[MetadataCacheResolver] Found {} in ProductMetadata (title: {:?})",
                    product_id,
                    meta.title
                );

                // Construct complete JSON with all fields
                let full_json = serde_json::json!({
                    "product_id": meta.external_id,
                    "id": meta.id,
                    "source": meta.source,
                    "title": meta.title,
                    "work_name": meta.title,
                    "creator": meta.creator,
                    "maker_name": meta.creator,
                    "circle": meta.creator,
                    "description": meta.description,
                    "release_date": meta.release_date,
                    "price": meta.price,
                    "currency": meta.currency,
                    "rating": meta.rating,
                    "rating_count": meta.rating_count,
                    "purchase_count": meta.purchase_count,
                    "favorite_count": meta.favorite_count,
                    "review_count": meta.review_count,
                    "file_size": meta.file_size,
                    "file_format": meta.file_format,
                    "age_rating": meta.age_rating,
                    "genres": meta.genres_json.as_ref()
                        .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                        .unwrap_or(serde_json::Value::Null),
                    "tags": meta.tags_json.as_ref()
                        .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                        .unwrap_or(serde_json::Value::Null),
                    "languages": meta.languages_json.as_ref()
                        .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                        .unwrap_or(serde_json::Value::Null),
                    "product_formats": meta.product_formats_json.as_ref()
                        .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                        .unwrap_or(serde_json::Value::Null),
                    "series_name": meta.series_name,
                    "illustrator": meta.illustrator,
                    "voice_actors": meta.voice_actors_json.as_ref()
                        .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                        .unwrap_or(serde_json::Value::Null),
                    "miscellaneous": meta.miscellaneous,
                    "update_info": meta.update_info,
                    "rankings": meta.rankings_json.as_ref()
                        .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                        .unwrap_or(serde_json::Value::Null),
                    "cached_at": meta.cached_at,
                });

                Some(full_json.to_string().into_bytes())
            }
            Ok(None) => None,
            Err(e) => {
                tracing::debug!(
                    "[MetadataCacheResolver] Error checking ProductMetadata: {}",
                    e
                );
                None
            }
        }
    }

    /// Try to get metadata from the legacy MetadataCache table
    fn try_legacy_table(&self, product_id: &str) -> Option<Vec<u8>> {
        match self.cache.get(product_id) {
            Ok(Some(meta)) => {
                tracing::info!(
                    "[MetadataCacheResolver] Found {} in legacy MetadataCache (title: {})",
                    product_id,
                    meta.title
                );

                // Construct JSON with legacy fields
                let full_json = serde_json::json!({
                    "product_id": meta.product_id,
                    "title": meta.title,
                    "work_name": meta.title,
                    "circle": meta.circle,
                    "maker_name": meta.circle,
                    "creator": meta.creator,
                    "price": meta.price,
                    "release_date": meta.release_date,
                    "description": meta.description,
                    "work_type": meta.work_type,
                    "file_format": meta.file_format,
                    "tags": meta.tags_json.as_ref()
                        .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                        .unwrap_or(serde_json::Value::Null),
                    "cached_at": meta.cached_at,
                });

                Some(full_json.to_string().into_bytes())
            }
            Ok(None) => None,
            Err(e) => {
                tracing::debug!(
                    "[MetadataCacheResolver] Error checking legacy MetadataCache: {}",
                    e
                );
                None
            }
        }
    }
}

impl DataSourceResolver for MetadataCacheResolver {
    fn try_resolve(&self, key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
        // Key format: "dlsite:json:RJ123456" or "dlsite:html:RJ123456" or just "RJ123456"
        let product_id = key
            .strip_prefix("dlsite:json:")
            .or_else(|| key.strip_prefix("dlsite:html:"))
            .unwrap_or(key);

        let is_html_key = key.starts_with("dlsite:html:");

        // For HTML keys, go directly to ContentCache (cacache)
        if is_html_key {
            if let Some(rm) = &self.resource_manager {
                if let Some(data) = rm.get(key) {
                    tracing::info!(
                        "[MetadataCacheResolver] Found HTML {} in ContentCache ({} bytes)",
                        key,
                        data.data.len()
                    );
                    return Ok(data.data);
                }
            }
            tracing::debug!("[MetadataCacheResolver] HTML {} not in ContentCache", key);
            return Err(ResolveError::NotFound);
        }

        // For JSON keys:
        // 1. Try ProductMetadata table
        if let Some(data) = self.try_new_table(product_id) {
            return Ok(data);
        }

        // 2. Try ContentCache for raw data
        if let Some(rm) = &self.resource_manager {
            if let Some(data) = rm.get(key) {
                tracing::info!(
                    "[MetadataCacheResolver] Found {} in ContentCache ({} bytes)",
                    key,
                    data.data.len()
                );
                return Ok(data.data);
            }
        }

        tracing::debug!("[MetadataCacheResolver] {} not in any cache", product_id);
        Err(ResolveError::NotFound)
    }

    fn has(&self, key: &str, _request: &DataRequest) -> bool {
        let product_id = key
            .strip_prefix("dlsite:json:")
            .or_else(|| key.strip_prefix("dlsite:html:"))
            .unwrap_or(key);

        // Check ProductMetadata table
        if let Some(cache_db) = &self.cache_db {
            let found = cache_db.with_connection(|conn| {
                Ok(
                    arclain_db::get_by_external_id(conn, MetadataSource::DLSite, product_id)
                        .ok()
                        .flatten()
                        .is_some(),
                )
            });
            if found.unwrap_or(false) {
                return true;
            }
        }

        // Check ContentCache
        if let Some(rm) = &self.resource_manager {
            if rm.has(key) {
                return true;
            }
        }

        false
    }
}
