//! Metadata caching operations

use super::HostFunctions;
use arclain_data::DataSource;
use tracing::{debug, error, warn};

impl HostFunctions {
    pub(super) fn impl_emit_metadata(&mut self, metadata_json: String) {
        debug!("[Plugin] Emitting metadata");

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
            "series": parsed["series_name"].as_str()
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
                // Save using Data API (best-effort — signal fires regardless)
                if let Err(e) = self
                    .data_service
                    .save_data(DataSource::MetadataStore, &id, &bytes)
                {
                    error!("Failed to save metadata via DataService: {}", e);
                } else {
                    debug!("[Cache SAVE] Saved {} via Data API", id);
                }

                // Always trigger the signal so the UI receives metadata even if
                // persistence failed (e.g. corrupt database with stale triggers)
                if let Some(ref signal) = self.metadata_signal {
                    signal.set(Some(parsed.clone()));
                    debug!("[Cache SAVE] Triggered metadata signal for {}", id);
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
        use std::collections::HashMap;

        let Some(lib_svc) = &self.library_service else {
            warn!("LibraryService not initialized");
            return vec![];
        };

        if ids.is_empty() {
            return vec![];
        }

        // Audit P13: the previous implementation looped
        // `lib_svc.get_metadata(&full_id)` once per id — N round-trips
        // through diesel + the per-DB `Mutex<Connection>`, fired on
        // every archive-list refresh. Now we do one
        // `WHERE id IN (?, ?, …)` and rebuild the per-id mapping
        // client-side.
        let full_ids: Vec<String> = ids.iter().map(|i| format!("dlsite:{}", i)).collect();
        let full_id_refs: Vec<&str> = full_ids.iter().map(String::as_str).collect();

        let rows = match lib_svc.get_many(&full_id_refs) {
            Ok(r) => r,
            Err(e) => {
                error!("get_metadata_summaries: get_many failed: {}", e);
                // Same fallback as the old loop's per-id error case —
                // return a placeholder summary per requested id so the
                // UI doesn't have to reason about partial failures.
                return ids
                    .into_iter()
                    .map(|external_id| MetadataSummary {
                        id: external_id,
                        title: None,
                        geo_blocked: false,
                    })
                    .collect();
            }
        };

        // Index the result rows so each requested external id can be
        // resolved without a second linear scan per id.
        let by_full_id: HashMap<&str, &arclain_core::ProductMetadata> =
            rows.iter().map(|m| (m.id.as_str(), m)).collect();

        ids.into_iter()
            .zip(full_ids.iter())
            .map(|(external_id, full_id)| match by_full_id.get(full_id.as_str()) {
                Some(meta) => MetadataSummary {
                    id: external_id,
                    title: meta.title.clone(),
                    geo_blocked: meta.geo_blocked,
                },
                None => MetadataSummary {
                    id: external_id,
                    title: None,
                    geo_blocked: false,
                },
            })
            .collect()
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
        let full_id = format!("{}:{}", source.to_lowercase(), product_id);

        // 0. Try gameta server first (if configured and available)
        if let Some(ref client) = self.gameta_client {
            match client.get_metadata(&source, &product_id) {
                Ok(Some(meta_resp)) => {
                    debug!(
                        "[get_product_metadata] Got {} from gameta server",
                        product_id
                    );
                    if let Ok(json) = serde_json::to_string(&meta_resp) {
                        return Some(json);
                    }
                }
                Ok(None) => {
                    // Not on server — try server-side fetch
                    match client.fetch_metadata(&source, &product_id, false) {
                        Ok(resp) => {
                            if let Some(meta) = resp.metadata {
                                debug!(
                                    "[get_product_metadata] Fetched {} via gameta server",
                                    product_id
                                );
                                if let Ok(json) = serde_json::to_string(&meta) {
                                    return Some(json);
                                }
                            } else {
                                debug!(
                                    "[get_product_metadata] Server fetch returned no metadata for {}, falling back",
                                    product_id
                                );
                            }
                        }
                        Err(_) => {
                            debug!(
                                "[get_product_metadata] Server fetch failed for {}, falling back",
                                product_id
                            );
                        }
                    }
                }
                Err(e) => {
                    debug!(
                        "[get_product_metadata] Gameta server error: {}, falling back",
                        e
                    );
                }
            }
        }

        // 1. Check metadata.sqlite first (fastest)
        if let Some(lib_svc) = &self.library_service {
            match lib_svc.get_metadata(&full_id) {
                Ok(Some(mut meta)) => {
                    self.try_repair_extras_from_html(
                        &mut meta,
                        &product_id,
                        &source,
                        &full_id,
                        lib_svc,
                    );

                    return serde_json::to_string(&meta)
                        .map_err(|e| {
                            // Audit finding M1: previously .ok() silently
                            // dropped serialization errors; the plugin
                            // saw "no metadata" with no debugging trail.
                            warn!(
                                "[get_product_metadata] failed to serialize metadata for {}: {}",
                                full_id, e
                            );
                            e
                        })
                        .ok();
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
                    return serde_json::to_string(&meta)
                        .map_err(|e| {
                            warn!(
                                "[get_product_metadata] failed to serialize JSON-cache metadata for {}: {}",
                                product_id, e
                            );
                            e
                        })
                        .ok();
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
                return serde_json::to_string(&meta)
                    .map_err(|e| {
                        warn!(
                            "[get_product_metadata] failed to serialize HTML-cache metadata for {}: {}",
                            product_id, e
                        );
                        e
                    })
                    .ok();
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

    /// Backfill missing image / extras data on a metadata row from
    /// cached HTML. Best-effort, all failure paths are silent — this
    /// runs on the read path and we don't want to error a UI fetch
    /// just because the HTML cache hasn't landed yet.
    ///
    /// Was previously inline inside `impl_get_product_metadata` at
    /// 11 levels of nesting. Pulled out so the read path reads
    /// top-to-bottom (server → DB → repair → return) instead of
    /// being buried under nested `if let / for / if`.
    fn try_repair_extras_from_html(
        &self,
        meta: &mut arclain_core::ProductMetadata,
        product_id: &str,
        source: &str,
        full_id: &str,
        lib_svc: &arclain_core::LibraryService,
    ) {
        // Skip the repair if cover_image is already populated.
        if meta
            .extras
            .get("cover_image")
            .and_then(|v| v.as_str())
            .is_some()
        {
            return;
        }

        let html_key = format!("{}:html:{}", source.to_lowercase(), product_id);
        let Some(html_bytes) = self.data_service.get_data(&html_key) else {
            return;
        };
        let html_str = String::from_utf8_lossy(&html_bytes);
        let Ok(repaired) =
            self.parse_dlsite_html_to_metadata(product_id, source, &html_str)
        else {
            return;
        };

        // Merge non-null image fields into existing extras, or replace
        // wholesale if extras isn't an object.
        if let Some(obj) = meta.extras.as_object_mut() {
            if let Some(rep) = repaired.extras.as_object() {
                merge_nonnull_into(obj, rep);
            }
        } else {
            meta.extras = repaired.extras;
        }

        if meta.tags.is_empty() {
            meta.tags = repaired.tags;
        }

        if let Err(e) = lib_svc.save_metadata(meta) {
            debug!(
                "[get_product_metadata] Failed to persist repaired extras: {}",
                e
            );
        }
        debug!("[get_product_metadata] Repaired extras for {}", full_id);
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
            "series": scraped.series,
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

}

/// Copy every non-null entry from `source` into `target`, overwriting
/// any existing key. Used by `try_repair_extras_from_html` to merge
/// scraped image fields into a stale `extras` JSON object.
fn merge_nonnull_into(
    target: &mut serde_json::Map<String, serde_json::Value>,
    source: &serde_json::Map<String, serde_json::Value>,
) {
    for (k, v) in source {
        if !v.is_null() {
            target.insert(k.clone(), v.clone());
        }
    }
}
