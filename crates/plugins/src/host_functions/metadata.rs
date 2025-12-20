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
        if let Some(cache) = &self.metadata_cache {
            // We need to parse the JSON to extract fields for the cache
            // This is a bit inefficient (parsing twice), but robust
            use arclain_db::CachedMetadata;

            // Try to parse basic info
            let parsed: serde_json::Value = match serde_json::from_str(&json) {
                Ok(v) => v,
                Err(e) => {
                    error!("Failed to parse metadata JSON for caching: {}", e);
                    return;
                }
            };

            info!("[Cache SAVE] Parsing metadata for {}", id);
            debug!("[Cache SAVE] Full JSON: {}", json);

            let title = parsed["title"].as_str().unwrap_or("Unknown").to_string();
            let circle = parsed["creator"].as_str().map(|s| s.to_string());

            info!(
                "[Cache SAVE] Extracted - title: {}, circle from 'creator': {:?}",
                title, circle
            );
            info!(
                "[Cache SAVE] Also checking 'circle' field: {:?}",
                parsed["circle"].as_str()
            );
            // Price is in dlsite.price as string
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

            let meta = CachedMetadata {
                product_id: id.clone(),
                title,
                circle,
                price,
                release_date,
                description,
                work_type,
                file_format,
                tags_json,
                raw_api_json: json,
                cached_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            };

            if let Err(e) = cache.save(&meta) {
                error!("Failed to save metadata to cache: {}", e);
            } else {
                self.log_network_activity(format!("Saved {} to cache", id));
            }
        } else {
            warn!("Metadata cache not initialized");
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
