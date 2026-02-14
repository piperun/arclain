//! Metadata caching operations

use super::HostFunctions;
use arclain_data::DataSource;
use tracing::{debug, error, warn};

impl HostFunctions {
    pub(super) fn impl_emit_metadata(&mut self, metadata_json: String) {
        // Store metadata for the host to process
        debug!("[Plugin] Emitting metadata");

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
    pub(super) fn impl_save_cached_metadata(&mut self, id: String, json: String) {
        // Parse the JSON to extract fields and convert to ProductMetadata
        let parsed: serde_json::Value = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to parse metadata JSON for caching: {}", e);
                return;
            }
        };

        debug!("[Cache SAVE] Parsing metadata for {}", id);

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

        use arclain_core::{MetadataSource, ProductMetadata};

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

        let genres: Vec<String> = parsed["genres"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let tags: Vec<String> = parsed["tags"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let languages: Vec<String> = parsed["languages"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // Check if geo-blocked
        let geo_blocked = parsed["geo_blocked"].as_bool().unwrap_or(false);

        // Build extras with image URLs, DLSite-specific fields, etc.
        let extras = serde_json::json!({
            "cover_image": parsed["cover_image"].as_str(),
            "screenshots": parsed["sample_images"].as_array()
                .or_else(|| parsed["screenshots"].as_array().filter(|arr|
                    arr.first().map(|v| v.is_string()).unwrap_or(true)
                ))
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                .unwrap_or_default(),
            "update_date": parsed["update_date"].as_str(),
            "authors": parsed["authors"].as_array(),
            "illustrators": parsed["illustrators"].as_array(),
            "scenarios": parsed["scenarios"].as_array(),
            "musicians": parsed["musicians"].as_array(),
            "writers": parsed["writers"].as_array(),
            "brand": parsed["brand"].as_str(),
            "publisher": parsed["publisher"].as_str(),
            "page_count": parsed["page_count"].as_i64(),
            // DLSite-specific fields stored in extras
            "series_name": parsed["series_name"].as_str()
                .or_else(|| parsed["dlsite"]["series"]["name"].as_str()),
            "illustrator": parsed["illustrator"].as_str(),
            "voice_actors": parsed["voice_actors"].as_array(),
            "product_formats": parsed["product_formats"].as_array(),
            "miscellaneous": parsed["miscellaneous"].as_str(),
            "update_info": parsed["update_info"].as_str(),
            "rankings": parsed["rankings"].as_object(),
        });

        let product = if geo_blocked {
            // Minimal metadata for geo-blocked items
            let mut meta = ProductMetadata::new(MetadataSource::DLSite, &id);
            meta.title = Some(title);
            meta.geo_blocked = true;
            meta.raw_api_response = Some(json);
            meta.cached_at = now_unix;
            meta
        } else {
            // Full metadata
            ProductMetadata {
                id: format!("dlsite:{}", id),
                source: MetadataSource::DLSite,
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
                genres,
                tags,
                languages,
                extras,
                raw_api_response: Some(json),
                raw_html: None,
                geo_blocked,
                cached_at: now_unix,
                updated_at: None,
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
                    debug!("[Cache SAVE] Saved {} via Data API", id);

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
        if let Some(lib_svc) = &self.library_service {
            // Assume DLSite source for now as this is likely invoked by dlsite plugin context
            match lib_svc.list_by_source(arclain_core::MetadataSource::DLSite) {
                Ok(entries) => {
                    debug!("[Cache] Listed {} cached entries", entries.len());
                    // Return external IDs (strip "source:")
                    entries
                        .into_iter()
                        .filter_map(|id| id.split_once(':').map(|(_, ext)| ext.to_string()))
                        .collect()
                }
                Err(e) => {
                    error!("Failed to list cached entries: {}", e);
                    vec![]
                }
            }
        } else {
            warn!("LibraryService not initialized");
            vec![]
        }
    }

    /// Batch query for metadata summaries (id, title, geo_blocked)
    pub(super) fn impl_get_metadata_summaries(
        &mut self,
        ids: Vec<String>,
    ) -> Vec<crate::arclain::plugin::host::MetadataSummary> {
        use crate::arclain::plugin::host::MetadataSummary;

        if let Some(lib_svc) = &self.library_service {
            ids.into_iter()
                .map(|external_id| {
                    let full_id = format!("dlsite:{}", external_id);

                    match lib_svc.get_metadata(&full_id) {
                        Ok(Some(meta)) => MetadataSummary {
                            id: external_id,
                            title: meta.title,
                            geo_blocked: meta.geo_blocked,
                        },
                        Ok(None) => MetadataSummary {
                            id: external_id,
                            title: None,
                            geo_blocked: false,
                        },
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
            warn!("LibraryService not initialized");
            vec![]
        }
    }

    /// Get full product metadata with fallback chain:
    /// 1. metadata.sqlite (instant)
    /// 2. JSON cache (parse + save to DB)
    /// 3. HTML cache (parse on host + save to DB)
    pub(super) fn impl_get_product_metadata(
        &mut self,
        product_id: String,
        source: String,
    ) -> Option<String> {
        use arclain_core::MetadataSource;

        let _source_enum = match source.to_lowercase().as_str() {
            "dlsite" => MetadataSource::DLSite,
            "steam" => MetadataSource::Steam,
            "itch" | "itchio" => MetadataSource::Itchio,
            _ => MetadataSource::Custom,
        };

        let full_id = format!("{}:{}", source.to_lowercase(), product_id);

        // 1. Check metadata.sqlite first (fastest) - trust the data
        if let Some(lib_svc) = &self.library_service {
            match lib_svc.get_metadata(&full_id) {
                Ok(Some(meta)) => {
                    return serde_json::to_string(&meta).ok();
                }
                Ok(None) => {
                    debug!(
                        "[get_product_metadata] Not in metadata.sqlite, checking caches..."
                    );
                }
                Err(e) => {
                    warn!(
                        "[get_product_metadata] Error reading metadata.sqlite: {}",
                        e
                    );
                }
            }
        }

        // 2. Check JSON cache (dlsite:json:ID) - one-time migration
        let json_key = format!("{}:json:{}", source.to_lowercase(), product_id);
        if let Some(json_bytes) = self.data_service.get_data(&json_key) {
            if let Ok(json_str) = String::from_utf8(json_bytes) {
                debug!("[get_product_metadata] Found JSON cache, migrating: {}", product_id);

                // Parse the raw API JSON into ProductMetadata
                if let Ok(meta) =
                    self.parse_dlsite_json_to_metadata(&product_id, &source, &json_str)
                {
                    // Save to metadata.sqlite for next time
                    if let Some(lib_svc) = &self.library_service {
                        if let Err(e) = lib_svc.save_metadata(&meta) {
                            warn!("[get_product_metadata] Failed to save to DB: {}", e);
                        }
                    }
                    return serde_json::to_string(&meta).ok();
                }
            }
        }

        // 3. Check HTML cache (dlsite:html:ID) - one-time migration
        let html_key = format!("{}:html:{}", source.to_lowercase(), product_id);
        if let Some(html_bytes) = self.data_service.get_data(&html_key) {
            let html_str = String::from_utf8_lossy(&html_bytes);
            debug!("[get_product_metadata] Found HTML cache, migrating: {}", product_id);

            // Parse HTML on host side using gameta_lib
            if let Ok(meta) =
                self.parse_dlsite_html_to_metadata(&product_id, &source, &html_str)
            {
                // Save to metadata.sqlite for next time
                if let Some(lib_svc) = &self.library_service {
                    if let Err(e) = lib_svc.save_metadata(&meta) {
                        warn!("[get_product_metadata] Failed to save to DB: {}", e);
                    }
                }
                return serde_json::to_string(&meta).ok();
            }
        }

        debug!(
            "[get_product_metadata] Not found in any cache: {}",
            product_id
        );
        None
    }

    /// Parse DLSite JSON response into ProductMetadata
    fn parse_dlsite_json_to_metadata(
        &self,
        product_id: &str,
        source: &str,
        json_str: &str,
    ) -> Result<arclain_core::ProductMetadata, String> {
        use arclain_core::ProductMetadata;

        // Parse the JSON
        let json: serde_json::Value =
            serde_json::from_str(json_str).map_err(|e| format!("Invalid JSON: {}", e))?;

        // API returns array, get first item
        let data = if let Some(arr) = json.as_array() {
            arr.first().cloned().unwrap_or(json.clone())
        } else {
            json
        };

        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let genres: Vec<String> = data["genre"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|g| g["name"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        Ok(ProductMetadata {
            id: format!("{}:{}", source.to_lowercase(), product_id),
            source: arclain_core::MetadataSource::DLSite,
            external_id: product_id.to_string(),
            title: data["work_name"].as_str().map(|s| s.to_string()),
            creator: data["maker_name"].as_str().map(|s| s.to_string()),
            description: data["intro_s"].as_str().map(|s| s.to_string()),
            release_date: data["regist_date"]
                .as_str()
                .map(|s| s.split_whitespace().next().unwrap_or(s).to_string()),
            price: data["price"]
                .as_i64()
                .or_else(|| data["price"].as_str().and_then(|s| s.parse().ok())),
            currency: Some("JPY".to_string()),
            rating: data["rate_average_2dp"].as_f64(),
            rating_count: data["rate_count"].as_i64(),
            purchase_count: data["dl_count"].as_i64(),
            favorite_count: data["wishlist_count"].as_i64(),
            review_count: data["review_count"].as_i64(),
            file_size: data["file_size"].as_str().map(|s| s.to_string()),
            file_format: data["file_type"].as_str().map(|s| s.to_string()),
            age_rating: data["age_category_string"].as_str().map(|s| s.to_string()),
            genres,
            tags: Vec::new(),
            languages: Vec::new(),
            extras: serde_json::Value::Null,
            raw_api_response: Some(json_str.to_string()),
            raw_html: None,
            geo_blocked: false,
            cached_at: now_unix,
            updated_at: None,
        })
    }

    /// Parse DLSite HTML into ProductMetadata (heavy lifting on host)
    fn parse_dlsite_html_to_metadata(
        &self,
        product_id: &str,
        source: &str,
        html_str: &str,
    ) -> Result<arclain_core::ProductMetadata, String> {
        use arclain_core::ProductMetadata;

        // Use gameta_lib's HTML parser (runs on host, not WASM)
        let scraped = gameta_lib::parsers::dlsite::parse_html(html_str)
            .ok_or_else(|| "Failed to parse HTML".to_string())?;

        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let extras = serde_json::json!({
            "cover_image": scraped.cover_image,
            "screenshots": scraped.screenshots,
            "authors": scraped.authors,
            "illustrators": scraped.illustrators,
            "scenarios": scraped.scenarios,
            "musicians": scraped.musicians,
            "writers": scraped.writers,
            "brand": scraped.brand,
            "publisher": scraped.publisher,
            "page_count": scraped.page_count,
            "update_date": scraped.update_date,
            "voice_actors": scraped.voice_actors,
            "series_name": scraped.series,
        });

        Ok(ProductMetadata {
            id: format!("{}:{}", source.to_lowercase(), product_id),
            source: arclain_core::MetadataSource::DLSite,
            external_id: product_id.to_string(),
            title: scraped.title,
            creator: scraped.circle,
            description: scraped.description,
            release_date: scraped.release_date,
            file_size: scraped.file_size,
            tags: scraped.tags,
            genres: scraped.genres,
            languages: Vec::new(),
            extras,
            geo_blocked: scraped.geo_blocked,
            cached_at: now_unix,
            price: None,
            currency: None,
            rating: None,
            rating_count: None,
            purchase_count: None,
            favorite_count: None,
            review_count: None,
            file_format: None,
            age_rating: None,
            raw_api_response: None,
            raw_html: None,
            updated_at: None,
        })
    }

    pub(super) fn impl_export_cache(&mut self) -> Result<String, String> {
        if let Some(lib_svc) = &self.library_service {
            use arclain_core::MetadataSource;
            match lib_svc.list_by_source(MetadataSource::DLSite) {
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
            Err("LibraryService not initialized".to_string())
        }
    }

    pub(super) fn impl_import_cache(&mut self) -> Result<String, String> {
        if let Some(lib_svc) = &self.library_service {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("JSON", &["json"])
                .pick_file()
            {
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| format!("Failed to read file: {}", e))?;

                // Try to parse as list of ProductMetadata
                let entries: Vec<arclain_core::ProductMetadata> = serde_json::from_str(&content)
                    .map_err(|e| {
                        format!(
                            "Invalid JSON format (expected ProductMetadata array): {}",
                            e
                        )
                    })?;

                let count = entries.len();
                let mut imported = 0;
                for meta in entries {
                    if let Err(e) = lib_svc.save_metadata(&meta) {
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
            Err("LibraryService not initialized".to_string())
        }
    }
}
