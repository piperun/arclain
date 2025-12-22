//! Metadata caching operations

use super::HostFunctions;
use crate::arclain::plugin::host::Host;
use tracing::{debug, error, info, warn};

impl HostFunctions {
    pub(super) fn impl_emit_metadata(&mut self, metadata_json: String) {
        // Store metadata for the host to process
        info!("[Plugin] Emitting metadata");
        debug!("[Plugin] Metadata JSON: {}", metadata_json);

        *self.emitted_metadata.lock() = Some(metadata_json);
    }

    pub(super) fn impl_get_cached_metadata(&mut self, id: String) -> Option<String> {
        if let Some(cache) = &self.metadata_cache {
            match cache.get(&id) {
                Ok(Some(meta)) => {
                    // Check if fresh (7 days)
                    match cache.is_fresh(&id, 7) {
                        Ok(true) => {
                            self.log_network_activity(format!("Cache HIT for {}", id));
                            info!(
                                "[Cache] Retrieved cached metadata for {}: title={}, circle={:?}",
                                id, meta.title, meta.circle
                            );
                            info!(
                                "[Cache] raw_api_json length: {} bytes",
                                meta.raw_api_json.len()
                            );
                            debug!("[Cache] raw_api_json content: {}", meta.raw_api_json);
                            Some(meta.raw_api_json)
                        }
                        Ok(false) => {
                            self.log_network_activity(format!("Cache STALE for {}", id));
                            None
                        }
                        Err(e) => {
                            error!("Failed to check cache freshness: {}", e);
                            None
                        }
                    }
                }
                Ok(None) => {
                    self.log_network_activity(format!("Cache MISS for {}", id));
                    None
                }
                Err(e) => {
                    error!("Failed to get cached metadata: {}", e);
                    None
                }
            }
        } else {
            warn!("Metadata cache not initialized");
            None
        }
    }

    pub(super) fn impl_save_cached_metadata(&mut self, id: String, json: String) {
        // Parse the JSON to extract fields
        let parsed: serde_json::Value = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to parse metadata JSON for caching: {}", e);
                return;
            }
        };

        info!("[Cache SAVE] Parsing metadata for {}", id);
        debug!("[Cache SAVE] Full JSON: {}", json);

        // Extract common fields
        let title = parsed["title"].as_str().unwrap_or("Unknown").to_string();
        let circle = parsed["circle"].as_str().map(|s| s.to_string());
        let creator = parsed["creator"].as_str().map(|s| s.to_string());
        let price = parsed["dlsite"]["price"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .map(|p| p as i64);
        let release_date = parsed["release_date"].as_str().map(|s| s.to_string());
        let description = parsed["description"].as_str().map(|s| s.to_string());
        let work_type = parsed["work_type"].as_str().map(|s| s.to_string());
        let file_format = parsed["file_format"].as_str().map(|s| s.to_string());
        let tags_json = parsed["tags"]
            .as_array()
            .map(|t| serde_json::to_string(t).unwrap_or_default());

        info!(
            "[Cache SAVE] Extracted - title: {}, circle: {:?}, creator: {:?}",
            title, circle, creator
        );

        // Save to ProductMetadata table (unified storage)
        if let Some(cache_db) = &self.cache_db {
            use arclain_db::{
                init_product_metadata_schema, save_product_metadata, MetadataSource,
                ProductMetadata,
            };
            use std::time::{SystemTime, UNIX_EPOCH};

            // Ensure table exists
            let _ = cache_db.with_connection(|conn| init_product_metadata_schema(conn));

            // Extract additional fields for new table
            let rating = parsed["rating"]
                .as_f64()
                .or_else(|| parsed["dlsite"]["rate_average_2dp"].as_f64());
            let rating_count = parsed["rating_count"]
                .as_i64()
                .or_else(|| parsed["dlsite"]["rate_count"].as_i64());
            let purchase_count = parsed["purchase_count"]
                .as_i64()
                .or_else(|| parsed["dlsite"]["dl_count"].as_i64());
            let favorite_count = parsed["favorite_count"]
                .as_i64()
                .or_else(|| parsed["dlsite"]["wishlist_count"].as_i64());
            let review_count = parsed["review_count"]
                .as_i64()
                .or_else(|| parsed["dlsite"]["review_count"].as_i64());
            let file_size = parsed["file_size"]
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| {
                    parsed["dlsite"]["file_size"]
                        .as_str()
                        .map(|s| s.to_string())
                });
            let age_rating = parsed["age_rating"]
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| {
                    parsed["dlsite"]["age_category"]
                        .as_str()
                        .map(|s| s.to_string())
                });

            // Genre/tags
            let genres_json = parsed["genres"]
                .as_array()
                .map(|t| serde_json::to_string(t).unwrap_or_default());
            let languages_json = parsed["languages"]
                .as_array()
                .map(|t| serde_json::to_string(t).unwrap_or_default());
            let product_formats_json = parsed["product_formats"]
                .as_array()
                .map(|t| serde_json::to_string(t).unwrap_or_default());

            // DLSite-specific
            let series_name = parsed["series_name"]
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| {
                    parsed["dlsite"]["series"]["name"]
                        .as_str()
                        .map(|s| s.to_string())
                });
            let illustrator = parsed["illustrator"].as_str().map(|s| s.to_string());
            let voice_actors_json = parsed["voice_actors"]
                .as_array()
                .map(|t| serde_json::to_string(t).unwrap_or_default());
            let miscellaneous = parsed["miscellaneous"].as_str().map(|s| s.to_string());
            let update_info = parsed["update_info"].as_str().map(|s| s.to_string());
            let rankings_json = parsed["rankings"]
                .as_object()
                .map(|o| serde_json::to_string(o).unwrap_or_default());

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            let product = ProductMetadata {
                id: format!("dlsite:{}", id),
                source: MetadataSource::DLSite.as_str().to_string(),
                external_id: id.clone(),
                title: Some(title),
                creator: creator.or(circle), // Use creator, fall back to circle
                description,
                release_date,
                price,
                currency: Some("JPY".to_string()),
                rating,
                rating_count,
                purchase_count,
                favorite_count,
                review_count,
                file_size,
                file_format,
                age_rating,
                genres_json,
                tags_json,
                languages_json,
                product_formats_json,
                series_name,
                illustrator,
                voice_actors_json,
                miscellaneous,
                update_info,
                rankings_json,
                extras_json: None,
                raw_api_response: Some(json),
                raw_html: None,
                cached_at: now,
                updated_at: None,
                last_accessed: None,
            };

            let result = cache_db.with_connection(|conn| save_product_metadata(conn, &product));

            match result {
                Ok(_) => {
                    info!("[Cache SAVE] Saved {} to ProductMetadata table", id);
                }
                Err(e) => {
                    error!("Failed to save metadata to ProductMetadata table: {}", e);
                }
            }
        }
    }

    pub(super) fn impl_list_cached_entries(&mut self) -> Vec<String> {
        if let Some(cache) = &self.metadata_cache {
            match cache.list_all() {
                Ok(entries) => {
                    info!("[Cache] Listed {} cached entries", entries.len());
                    entries
                }
                Err(e) => {
                    error!("Failed to list cached entries: {}", e);
                    vec![]
                }
            }
        } else {
            warn!("Metadata cache not initialized");
            vec![]
        }
    }

    pub(super) fn impl_export_cache(&mut self) -> Result<String, String> {
        if let Some(cache) = &self.metadata_cache {
            match cache.list_all() {
                Ok(entries) => {
                    let mut export_data = Vec::new();
                    for id in entries {
                        if let Ok(Some(meta)) = cache.get(&id) {
                            export_data.push(meta);
                        }
                    }

                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name("dlsite_cache_export.json")
                        .add_filter("JSON", &["json"])
                        .save_file()
                    {
                        let json = serde_json::to_string_pretty(&export_data)
                            .map_err(|e| format!("Serialization failed: {}", e))?;

                        std::fs::write(&path, json)
                            .map_err(|e| format!("Failed to write file: {}", e))?;

                        Ok(format!(
                            "Exported {} entries to {:?}",
                            export_data.len(),
                            path
                        ))
                    } else {
                        Err("Export cancelled".to_string())
                    }
                }
                Err(e) => Err(format!("Failed to list entries: {}", e)),
            }
        } else {
            Err("Cache not initialized".to_string())
        }
    }

    pub(super) fn impl_import_cache(&mut self) -> Result<String, String> {
        if let Some(cache) = &self.metadata_cache {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("JSON", &["json"])
                .pick_file()
            {
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| format!("Failed to read file: {}", e))?;

                let entries: Vec<arclain_db::CachedMetadata> = serde_json::from_str(&content)
                    .map_err(|e| format!("Invalid JSON format: {}", e))?;

                let count = entries.len();
                let mut imported = 0;
                for meta in entries {
                    if let Err(e) = cache.save(&meta) {
                        error!("Failed to import entry {}: {}", meta.product_id, e);
                    } else {
                        imported += 1;
                    }
                }

                Ok(format!(
                    "Imported {}/{} entries from {:?}",
                    imported, count, path
                ))
            } else {
                Err("Import cancelled".to_string())
            }
        } else {
            Err("Cache not initialized".to_string())
        }
    }
}
