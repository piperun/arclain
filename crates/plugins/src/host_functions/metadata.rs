//! Metadata caching operations

use super::HostFunctions;
use crate::arclain::plugin::host::Host;
use arclain_data::DataSource;
use tracing::{debug, error, info, warn};

impl HostFunctions {
    pub(super) fn impl_emit_metadata(&mut self, metadata_json: String) {
        // Store metadata for the host to process
        info!("[Plugin] Emitting metadata");
        debug!("[Plugin] Metadata JSON: {}", metadata_json);

        *self.emitted_metadata.lock() = Some(metadata_json.clone());

        // Auto-persist to MetadataStore
        // Flatten the JSON to get the ID
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&metadata_json) {
            if let Some(id_str) = parsed["product_id"].as_str() {
                let id = id_str.to_string();
                self.impl_save_cached_metadata(id, metadata_json);
            } else {
                warn!("[Plugin] Emitted metadata missing 'product_id', cannot persist.");
            }
        } else {
            error!("[Plugin] Failed to parse emitted metadata JSON");
        }
    }

    /// Legacy - now handled by Data API
    #[allow(dead_code)]
    pub(super) fn impl_get_cached_metadata(&mut self, id: String) -> Option<String> {
        // Using Data API to retrieve metadata
        // Note: This returns ProductMetadata JSON, not necessarily the raw API response.
        if let Some(data) = self.data_service.get_data(&id) {
            match String::from_utf8(data) {
                Ok(s) => {
                    self.log_network_activity(format!("Cache HIT for {}", id));
                    // Basic stats for log
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&s) {
                        info!(
                            "[Cache] Retrieved metadata for {}: title={}",
                            id,
                            json["title"].as_str().unwrap_or("?")
                        );
                    }
                    Some(s)
                }
                Err(e) => {
                    error!("[Cache] Failed to convert data to string: {}", e);
                    None
                }
            }
        } else {
            self.log_network_activity(format!("Cache MISS for {}", id));
            None
        }
    }

    /// Legacy - now handled by Data API
    #[allow(dead_code)]
    pub(super) fn impl_save_cached_metadata(&mut self, id: String, json: String) {
        // Parse the JSON to extract fields and convert to ProductMetadata
        let parsed: serde_json::Value = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to parse metadata JSON for caching: {}", e);
                return;
            }
        };

        info!("[Cache SAVE] Parsing metadata for {}", id);

        // Logic to reconstruct ProductMetadata
        // We reuse the parsing logic from before just to be safe and consistent

        let title = parsed["title"].as_str().unwrap_or("Unknown").to_string();
        let circle = parsed["circle"].as_str().map(|s| s.to_string());
        let creator = parsed["creator"].as_str().map(|s| s.to_string());
        let price = parsed["dlsite"]["price"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .map(|p| p as i64);
        let release_date = parsed["release_date"].as_str().map(|s| s.to_string());
        let description = parsed["description"].as_str().map(|s| s.to_string());
        let file_format = parsed["file_format"].as_str().map(|s| s.to_string());
        let tags_json = parsed["tags"]
            .as_array()
            .map(|t| serde_json::to_string(t).unwrap_or_default());

        use arclain_db::{MetadataSource, ProductMetadata};

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

        let genres_json = parsed["genres"]
            .as_array()
            .map(|t| serde_json::to_string(t).unwrap_or_default());
        let languages_json = parsed["languages"]
            .as_array()
            .map(|t| serde_json::to_string(t).unwrap_or_default());
        let product_formats_json = parsed["product_formats"]
            .as_array()
            .map(|t| serde_json::to_string(t).unwrap_or_default());

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

        let now = chrono::Utc::now().to_rfc3339();

        // Check if geo-blocked
        let geo_blocked = parsed["geo_blocked"].as_bool();

        let is_blocked = geo_blocked.unwrap_or(false);

        let product = if is_blocked {
            // Minimal metadata for geo-blocked items
            ProductMetadata {
                id: format!("dlsite:{}", id),
                source: MetadataSource::DLSite.as_str().to_string(),
                external_id: id.clone(),
                title: Some(title),
                geo_blocked,
                cached_at: now,
                raw_api_response: Some(json), // Keep raw data for debugging
                // All other fields None/Default
                ..Default::default()
            }
        } else {
            // Full metadata
            ProductMetadata {
                id: format!("dlsite:{}", id),
                source: MetadataSource::DLSite.as_str().to_string(),
                external_id: id.clone(),
                title: Some(title),
                creator: creator.or(circle),
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
                geo_blocked,
                cached_at: now,
                updated_at: None,
                last_accessed: None,
            }
        };

        // Serialize the ProductMetadata
        match serde_json::to_vec(&product) {
            Ok(bytes) => {
                // Save using Data API
                if let Err(e) = self
                    .data_service
                    .save_data(DataSource::MetadataStore, &id, &bytes)
                {
                    error!("Failed to save metadata via DataService: {}", e);
                } else {
                    info!("[Cache SAVE] Saved {} via Data API", id);

                    // Trigger reactive signal to notify UI of metadata update
                    if let Some(ref signal) = self.metadata_signal {
                        signal.set(Some(parsed.clone()));
                        debug!("[Cache SAVE] Triggered metadata signal for {}", id);
                    }
                }
            }
            Err(e) => error!("Failed to serialize ProductMetadata: {}", e),
        }
    }

    pub(super) fn impl_list_cached_entries(&mut self) -> Vec<String> {
        if let Some(store) = &self.metadata_store {
            // Assume DLSite source for now as this is likely invoked by dlsite plugin context
            match store.list_by_source(arclain_db::MetadataSource::DLSite) {
                Ok(entries) => {
                    debug!("[Cache] Listed {} cached entries", entries.len());
                    // Return external IDs
                    entries.into_iter().map(|m| m.external_id).collect()
                }
                Err(e) => {
                    error!("Failed to list cached entries: {}", e);
                    vec![]
                }
            }
        } else {
            warn!("Metadata store not initialized");
            vec![]
        }
    }

    /// Batch query for metadata summaries (id, title, geo_blocked)
    pub(super) fn impl_get_metadata_summaries(
        &mut self,
        ids: Vec<String>,
    ) -> Vec<crate::arclain::plugin::host::MetadataSummary> {
        use crate::arclain::plugin::host::MetadataSummary;

        if let Some(store) = &self.metadata_store {
            ids.into_iter()
                .map(|external_id| {
                    // Format ID as expected by MetadataStore: "source:external_id"
                    let full_id = format!("dlsite:{}", external_id);

                    match store.get(&full_id) {
                        Ok(Some(meta)) => MetadataSummary {
                            id: external_id,
                            title: meta.title,
                            geo_blocked: meta.geo_blocked.unwrap_or(false),
                        },
                        Ok(None) => {
                            // Not found - return minimal summary
                            MetadataSummary {
                                id: external_id,
                                title: None,
                                geo_blocked: false,
                            }
                        }
                        Err(e) => {
                            error!("Failed to get metadata for {}: {}", external_id, e);
                            MetadataSummary {
                                id: external_id,
                                title: None,
                                geo_blocked: false,
                            }
                        }
                    }
                })
                .collect()
        } else {
            warn!("Metadata store not initialized");
            vec![]
        }
    }

    pub(super) fn impl_export_cache(&mut self) -> Result<String, String> {
        if let Some(store) = &self.metadata_store {
            use arclain_db::MetadataSource;
            match store.list_by_source(MetadataSource::DLSite) {
                Ok(entries) => {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name("arclain_metadata_export.json")
                        .add_filter("JSON", &["json"])
                        .save_file()
                    {
                        let json = serde_json::to_string_pretty(&entries)
                            .map_err(|e| format!("Serialization failed: {}", e))?;

                        std::fs::write(&path, json)
                            .map_err(|e| format!("Failed to write file: {}", e))?;

                        Ok(format!("Exported {} entries to {:?}", entries.len(), path))
                    } else {
                        Err("Export cancelled".to_string())
                    }
                }
                Err(e) => Err(format!("Failed to list entries: {}", e)),
            }
        } else {
            Err("Metadata store not initialized".to_string())
        }
    }

    pub(super) fn impl_import_cache(&mut self) -> Result<String, String> {
        if let Some(store) = &self.metadata_store {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("JSON", &["json"])
                .pick_file()
            {
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| format!("Failed to read file: {}", e))?;

                // Try to parse as list of ProductMetadata
                let entries: Vec<arclain_db::ProductMetadata> = serde_json::from_str(&content)
                    .map_err(|e| {
                        format!(
                            "Invalid JSON format (expected ProductMetadata array): {}",
                            e
                        )
                    })?;

                let count = entries.len();
                let mut imported = 0;
                for meta in entries {
                    if let Err(e) = store.save(&meta) {
                        error!("Failed to import entry {}: {}", meta.id, e);
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
            Err("Metadata store not initialized".to_string())
        }
    }
}
