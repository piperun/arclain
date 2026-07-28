//! Metadata caching operations

use super::HostFunctions;
use arclain_data::DataSource;
use std::collections::HashSet;
use std::io::Write;
use std::time::{Duration, Instant};
use tracing::{debug, error, warn};

const MAX_METADATA_PRODUCT_ID_BYTES: usize = 256;
const MAX_METADATA_SOURCE_BYTES: usize = 256;
const MAX_METADATA_SUMMARY_IDS: usize = 256;
const MAX_CACHED_ENTRY_IDS: usize = 1024;
const MAX_METADATA_COLLECTION_BYTES: usize = 1024 * 1024;
pub(super) const MAX_CACHED_METADATA_PAGE_ITEMS: usize = 256;
pub(super) const MAX_METADATA_WRITES_PER_MINUTE: usize = 120;
pub(super) const MAX_METADATA_DISTINCT_IDS_PER_SESSION: usize = 1024;
pub(super) const MAX_METADATA_BYTES_PER_SESSION: usize = 64 * 1024 * 1024;
const METADATA_WRITE_WINDOW: Duration = Duration::from_secs(60);

pub(super) struct MetadataWriteBudget {
    window_started: Instant,
    writes_in_window: usize,
    accepted_bytes: usize,
    distinct_ids: HashSet<String>,
}

impl Default for MetadataWriteBudget {
    fn default() -> Self {
        Self {
            window_started: Instant::now(),
            writes_in_window: 0,
            accepted_bytes: 0,
            distinct_ids: HashSet::new(),
        }
    }
}

impl MetadataWriteBudget {
    fn admit(&mut self, id: &str, bytes: usize) -> bool {
        self.admit_at(id, bytes, Instant::now())
    }

    fn admit_at(&mut self, id: &str, bytes: usize, now: Instant) -> bool {
        if now
            .checked_duration_since(self.window_started)
            .is_some_and(|elapsed| elapsed >= METADATA_WRITE_WINDOW)
        {
            self.window_started = now;
            self.writes_in_window = 0;
        }
        if self.writes_in_window >= MAX_METADATA_WRITES_PER_MINUTE {
            return false;
        }
        if !self.distinct_ids.contains(id)
            && self.distinct_ids.len() >= MAX_METADATA_DISTINCT_IDS_PER_SESSION
        {
            return false;
        }
        let Some(next_bytes) = self.accepted_bytes.checked_add(bytes) else {
            return false;
        };
        if next_bytes > MAX_METADATA_BYTES_PER_SESSION {
            return false;
        }

        self.writes_in_window += 1;
        self.accepted_bytes = next_bytes;
        self.distinct_ids.insert(id.to_owned());
        true
    }
}

fn parse_metadata_source(source: &str) -> Result<arclain_core::MetadataSource, String> {
    if source.is_empty() || source.len() > MAX_METADATA_SOURCE_BYTES {
        return Err("metadata source is empty or exceeds 256 bytes".to_string());
    }
    arclain_core::MetadataSource::from_str(source)
        .ok_or_else(|| "unsupported metadata source".to_string())
}

struct BoundedJsonWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl BoundedJsonWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }
}

impl Write for BoundedJsonWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(buffer.len()) > self.limit {
            return Err(std::io::Error::other(
                "serialized metadata exceeds host limit",
            ));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn serialize_metadata_json<T: serde::Serialize>(value: &T, limit: usize) -> Option<String> {
    let mut writer = BoundedJsonWriter::new(limit);
    serde_json::to_writer(&mut writer, value).ok()?;
    String::from_utf8(writer.bytes).ok()
}

fn cached_entries_with_query(
    query: impl FnOnce(usize) -> anyhow::Result<Vec<String>>,
) -> Vec<String> {
    let entries = match query(MAX_CACHED_ENTRY_IDS) {
        Ok(entries) => entries,
        Err(error) => {
            error!("Failed to list cached entries: {}", error);
            return Vec::new();
        }
    };

    let mut retained_bytes = 0_usize;
    let mut bounded = Vec::with_capacity(entries.len().min(MAX_CACHED_ENTRY_IDS));
    for id in entries.into_iter().take(MAX_CACHED_ENTRY_IDS) {
        if !id.contains(':') {
            continue;
        }
        let Some(external_id) = bounded_external_id(&id, &mut retained_bytes) else {
            break;
        };
        bounded.push(external_id.to_owned());
    }
    bounded
}

fn bounded_external_id<'a>(full_id: &'a str, retained_bytes: &mut usize) -> Option<&'a str> {
    let (_, external_id) = full_id.split_once(':')?;
    let next = retained_bytes.checked_add(external_id.len())?;
    if next > MAX_METADATA_COLLECTION_BYTES {
        return None;
    }
    *retained_bytes = next;
    Some(external_id)
}

fn cached_metadata_page_with_query(
    source: &str,
    offset: u32,
    limit: u32,
    query: impl FnOnce(arclain_core::MetadataSource, usize, usize) -> anyhow::Result<Vec<String>>,
) -> Result<Vec<String>, String> {
    let source = parse_metadata_source(source)?;
    let offset = usize::try_from(offset).map_err(|_| "metadata page offset is invalid")?;
    let limit = usize::try_from(limit).map_err(|_| "metadata page limit is invalid")?;
    if limit > MAX_CACHED_METADATA_PAGE_ITEMS {
        return Err(format!(
            "metadata page limit exceeds {MAX_CACHED_METADATA_PAGE_ITEMS} entries"
        ));
    }
    let rows = query(source, offset, limit).map_err(|error| {
        error!("Failed to list cached metadata page: {error}");
        "failed to list cached metadata".to_string()
    })?;
    let expected_prefix = format!("{}:", source.as_str());
    let mut retained_bytes = 0usize;
    let mut page = Vec::with_capacity(rows.len().min(limit));
    for row in rows.into_iter().take(limit) {
        let Some(external_id) = row.strip_prefix(&expected_prefix) else {
            continue;
        };
        if external_id.len() > MAX_METADATA_PRODUCT_ID_BYTES {
            continue;
        }
        let Some(next) = retained_bytes.checked_add(external_id.len()) else {
            return Err("metadata page text budget overflowed".to_string());
        };
        if next > MAX_METADATA_COLLECTION_BYTES {
            return Err("metadata page exceeds the 1 MiB text budget".to_string());
        }
        retained_bytes = next;
        page.push(external_id.to_owned());
    }
    Ok(page)
}

fn metadata_summaries_with_query(
    ids: Vec<String>,
    query: impl FnOnce(&[&str], usize) -> anyhow::Result<Vec<arclain_core::MetadataSummary>>,
) -> Vec<crate::arclain::plugin::host::MetadataSummary> {
    metadata_summaries_for_source_with_query("dlsite", ids, query).unwrap_or_default()
}

fn metadata_summaries_for_source_with_query(
    source: &str,
    ids: Vec<String>,
    query: impl FnOnce(&[&str], usize) -> anyhow::Result<Vec<arclain_core::MetadataSummary>>,
) -> Result<Vec<crate::arclain::plugin::host::MetadataSummary>, String> {
    use crate::arclain::plugin::host::MetadataSummary;
    use std::collections::HashMap;

    let source = parse_metadata_source(source)?;
    if ids.is_empty() || ids.len() > MAX_METADATA_SUMMARY_IDS {
        return if ids.is_empty() {
            Ok(Vec::new())
        } else {
            Err("metadata summary batch exceeds 256 ids".to_string())
        };
    }
    let Some(input_id_bytes) = ids.iter().try_fold(0_usize, |total, id| {
        (id.len() <= MAX_METADATA_PRODUCT_ID_BYTES)
            .then(|| total.checked_add(id.len()))
            .flatten()
    }) else {
        return Err("metadata summary contains an oversized id".to_string());
    };
    if input_id_bytes > MAX_METADATA_COLLECTION_BYTES {
        return Err("metadata summary input exceeds the 1 MiB budget".to_string());
    }

    let full_ids: Vec<String> = ids
        .iter()
        .map(|id| format!("{}:{id}", source.as_str()))
        .collect();
    let full_id_refs: Vec<&str> = full_ids.iter().map(String::as_str).collect();
    let rows = match query(&full_id_refs, MAX_METADATA_SUMMARY_IDS) {
        Ok(rows) => rows,
        Err(error) => {
            error!("get_metadata_summaries: bounded query failed: {}", error);
            return Ok(ids
                .into_iter()
                .map(|id| MetadataSummary {
                    id,
                    title: None,
                    geo_blocked: false,
                })
                .collect());
        }
    };
    let by_full_id: HashMap<&str, &arclain_core::MetadataSummary> = rows
        .iter()
        .map(|metadata| (metadata.id.as_str(), metadata))
        .collect();
    let mut remaining_title_bytes = MAX_METADATA_COLLECTION_BYTES - input_id_bytes;

    Ok(ids
        .into_iter()
        .zip(full_ids.iter())
        .map(|(id, full_id)| {
            let metadata = by_full_id.get(full_id.as_str()).copied();
            let title = metadata
                .and_then(|metadata| metadata.title.as_deref())
                .and_then(|title| {
                    if title.len() > remaining_title_bytes {
                        return None;
                    }
                    remaining_title_bytes -= title.len();
                    Some(title.to_owned())
                });
            MetadataSummary {
                id,
                title,
                geo_blocked: metadata.is_some_and(|metadata| metadata.geo_blocked),
            }
        })
        .collect())
}

impl HostFunctions {
    pub(super) fn impl_emit_metadata(&mut self, metadata_json: String) -> bool {
        debug!("[Plugin] Emitting metadata");

        if metadata_json.len() > crate::types::MAX_PLUGIN_METADATA_BYTES {
            return false;
        }

        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&metadata_json) else {
            return false;
        };
        let source = parsed["source"].as_str().unwrap_or("dlsite").to_owned();
        self.impl_emit_metadata_for_source_parsed(source, metadata_json, parsed)
    }

    pub(super) fn impl_emit_metadata_for_source(
        &mut self,
        source: String,
        metadata_json: String,
    ) -> bool {
        if metadata_json.len() > crate::types::MAX_PLUGIN_METADATA_BYTES {
            return false;
        }
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&metadata_json) else {
            return false;
        };
        self.impl_emit_metadata_for_source_parsed(source, metadata_json, parsed)
    }

    fn impl_emit_metadata_for_source_parsed(
        &mut self,
        source: String,
        metadata_json: String,
        parsed: serde_json::Value,
    ) -> bool {
        let Ok(source_kind) = parse_metadata_source(&source) else {
            return false;
        };
        if parsed["source"].as_str().is_some_and(|payload_source| {
            parse_metadata_source(payload_source).ok() != Some(source_kind)
        }) {
            return false;
        }
        let Some(id) = parsed["product_id"].as_str() else {
            return false;
        };
        if id.is_empty() || id.len() > MAX_METADATA_PRODUCT_ID_BYTES {
            return false;
        }

        let id = id.to_owned();
        let full_id = format!("{}:{id}", source_kind.as_str());
        if !self
            .metadata_write_budget
            .admit(&full_id, metadata_json.len())
        {
            return false;
        }
        self.impl_save_cached_metadata(source_kind, id, metadata_json, parsed)
    }

    pub(super) fn impl_save_cached_metadata(
        &mut self,
        source: arclain_core::MetadataSource,
        id: String,
        json: String,
        parsed: serde_json::Value,
    ) -> bool {
        debug!("[Cache SAVE] Parsing plugin metadata");

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

        // Preserve provider-defined extras and add host-controlled provenance.
        let mut extras = parsed["extras"].as_object().cloned().unwrap_or_default();
        extras.insert(
            "_arclain".to_string(),
            serde_json::json!({"emitted_by_plugin": self.plugin_id.as_str()}),
        );
        let dlsite_extras = serde_json::json!({
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
        if source == MetadataSource::DLSite {
            if let Some(provider_extras) = dlsite_extras.as_object() {
                merge_nonnull_into(&mut extras, provider_extras);
            }
        }
        let extras = serde_json::Value::Object(extras);

        let product = if geo_blocked {
            // Minimal metadata for geo-blocked items
            let mut meta = ProductMetadata::new(source, &id);
            meta.title = Some(title);
            meta.geo_blocked = true;
            meta.extras = extras;
            meta.raw_api_response = Some(json);
            meta.cached_at = now_unix;
            meta
        } else {
            // Full metadata
            ProductMetadata {
                id: format!("{}:{}", source.as_str(), id),
                source,
                external_id: id.clone(),
                title: Some(title),
                creator: creator.or(circle),
                description,
                release_date,
                price,
                currency: parsed["currency"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| (source == MetadataSource::DLSite).then(|| "JPY".to_string())),
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
        let mut writer = BoundedJsonWriter::new(
            self.data_service
                .materialization_limit()
                .min(crate::types::MAX_PLUGIN_METADATA_BYTES),
        );
        match serde_json::to_writer(&mut writer, &product) {
            Ok(()) => {
                // Save using Data API (best-effort — signal fires regardless)
                if let Err(e) =
                    self.data_service
                        .save_data(DataSource::MetadataStore, &id, &writer.bytes)
                {
                    error!("Failed to save metadata via DataService: {}", e);
                } else {
                    debug!("[Cache SAVE] Saved plugin metadata via Data API");
                }

                // Always trigger the signal so the UI receives metadata even if
                // persistence failed (e.g. corrupt database with stale triggers).
                //
                // Per-event context wins: if the dispatch worker installed
                // one, this emit is happening inside a plugin event handler
                // that was queued for a specific tab — the metadata must
                // land on *that* tab's signal, never on whichever tab the
                // user is currently looking at.
                //
                // Without the context (panel render, manual UI emit), fall
                // through to the bridge. Two branches from there, both
                // needed to match the pre-bridge behavior faithfully:
                //
                // - The active tab has an archive open
                //   (`active_archive_session_id` is `Some`): resolve and
                //   write via `set_session_metadata`, the same session-id
                //   path the event-context branch above uses. Equivalent
                //   to "the active tab", since the id it resolves *is* the
                //   active tab's own session.
                // - The active tab has no archive open (`None`): fall back
                //   to `set_active_tab_metadata`, which writes directly to
                //   whichever tab is active with no session-id resolution
                //   at all. This restores the original, pre-decoupling
                //   behavior exactly: the removed `metadata_signal()`
                //   method wrote to the active tab's signal
                //   unconditionally, regardless of whether an archive was
                //   open in it. Resolving only through
                //   `active_archive_session_id` (dropping the write when
                //   it's `None`) was a real regression this branch fixes.
                if let Some(ref ctx) = self.event_context {
                    if let Some(ref bridge) = self.active_tab {
                        bridge.set_session_metadata(ctx.archive_session_id, Some(parsed.clone()));
                        debug!(
                            "[Cache SAVE] Triggered metadata signal via event context session id"
                        );
                    }
                } else if let Some(ref bridge) = self.active_tab {
                    match bridge.active_archive_session_id() {
                        Some(session_id) => {
                            bridge.set_session_metadata(session_id, Some(parsed.clone()));
                            debug!("[Cache SAVE] Triggered metadata signal via active session id");
                        }
                        None => {
                            bridge.set_active_tab_metadata(Some(parsed.clone()));
                            debug!(
                                "[Cache SAVE] Triggered metadata signal via active-tab fallback \
                                 (no archive open in the active tab)"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to serialize ProductMetadata: {}", e);
                return false;
            }
        }
        true
    }

    pub(super) fn impl_list_cached_entries(&mut self) -> Vec<String> {
        if let Some(lib_svc) = &self.library_service {
            let entries = cached_entries_with_query(|limit| {
                lib_svc.list_by_source_limited(arclain_core::MetadataSource::DLSite, limit)
            });
            debug!("[Cache] Listed {} bounded cached entries", entries.len());
            entries
        } else {
            warn!("LibraryService not initialized");
            vec![]
        }
    }

    pub(super) fn impl_cached_metadata_count(&self, source: String) -> Result<u64, String> {
        let source = parse_metadata_source(&source)?;
        let library = self
            .library_service
            .as_ref()
            .ok_or_else(|| "LibraryService not initialized".to_string())?;
        library.count_by_source(source).map_err(|error| {
            error!("Failed to count cached metadata: {error}");
            "failed to count cached metadata".to_string()
        })
    }

    pub(super) fn impl_list_cached_metadata(
        &self,
        source: String,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<String>, String> {
        let library = self
            .library_service
            .as_ref()
            .ok_or_else(|| "LibraryService not initialized".to_string())?;
        cached_metadata_page_with_query(&source, offset, limit, |source, offset, limit| {
            library.list_by_source_page(source, offset, limit)
        })
    }

    /// Batch query for metadata summaries (id, title, geo_blocked)
    pub(super) fn impl_get_metadata_summaries(
        &mut self,
        ids: Vec<String>,
    ) -> Vec<crate::arclain::plugin::host::MetadataSummary> {
        let Some(lib_svc) = &self.library_service else {
            warn!("LibraryService not initialized");
            return Vec::new();
        };
        metadata_summaries_with_query(ids, |full_ids, limit| {
            lib_svc.get_summaries_limited(full_ids, limit)
        })
    }

    pub(super) fn impl_get_metadata_summaries_for_source(
        &self,
        source: String,
        ids: Vec<String>,
    ) -> Result<Vec<crate::arclain::plugin::host::MetadataSummary>, String> {
        let library = self
            .library_service
            .as_ref()
            .ok_or_else(|| "LibraryService not initialized".to_string())?;
        metadata_summaries_for_source_with_query(&source, ids, |full_ids, limit| {
            library.get_summaries_limited(full_ids, limit)
        })
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
        if product_id.len() > MAX_METADATA_PRODUCT_ID_BYTES
            || source.len() > MAX_METADATA_SOURCE_BYTES
        {
            return None;
        }
        let source_kind = parse_metadata_source(&source).ok()?;
        let source = source_kind.as_str().to_string();
        let json_limit = self
            .data_service
            .materialization_limit()
            .min(crate::types::MAX_PLUGIN_METADATA_BYTES);
        let full_id = format!("{}:{}", source.to_lowercase(), product_id);

        // 1. Check metadata.sqlite first (fastest)
        if let Some(lib_svc) = &self.library_service {
            match lib_svc.get_metadata(&full_id) {
                Ok(Some(mut meta)) => {
                    if source_kind == arclain_core::MetadataSource::DLSite {
                        self.try_repair_extras_from_html(&mut meta, &product_id, &source, lib_svc);
                    }

                    return serialize_metadata_json(&meta, json_limit);
                }
                Ok(None) => {
                    debug!("[get_product_metadata] Not in metadata.sqlite, checking caches...");
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
        if source_kind == arclain_core::MetadataSource::DLSite {
            let json_key = format!("{}:json:{}", source, product_id);
            let json_request = self.readable_cache_request(&json_key);
            if let Some(json_bytes) = self.data_service.get_data_for_request(&json_request) {
                if let Ok(json_str) = String::from_utf8(json_bytes) {
                    debug!("[get_product_metadata] Found JSON cache; migrating");

                    if let Ok(meta) =
                        self.parse_dlsite_json_to_metadata(&product_id, &source, &json_str)
                    {
                        if self
                            .check_capability(crate::types::PluginCapability::ArchiveMetadataWrite)
                        {
                            if let Some(lib_svc) = &self.library_service {
                                if let Err(e) = lib_svc.save_metadata(&meta) {
                                    warn!("[get_product_metadata] Failed to save to DB: {}", e);
                                }
                            }
                        }
                        return serialize_metadata_json(&meta, json_limit);
                    }
                }
            }

            // Only the DLSite provider has a host-side raw HTML parser. Other
            // sources resolve from structured DB rows or their Gameta provider.
            let html_key = format!("{}:html:{}", source, product_id);
            let html_request = self.readable_cache_request(&html_key);
            if let Some(html_bytes) = self.data_service.get_data_for_request(&html_request) {
                let html_str = String::from_utf8_lossy(&html_bytes);
                debug!("[get_product_metadata] Found HTML cache; migrating");

                if let Ok(meta) =
                    self.parse_dlsite_html_to_metadata(&product_id, &source, &html_str)
                {
                    if self.check_capability(crate::types::PluginCapability::ArchiveMetadataWrite) {
                        if let Some(lib_svc) = &self.library_service {
                            if let Err(e) = lib_svc.save_metadata(&meta) {
                                warn!("[get_product_metadata] Failed to save to DB: {}", e);
                            }
                        }
                    }
                    return serialize_metadata_json(&meta, json_limit);
                }
            }
        }

        // 4. Try Gameta only after every local store misses. The policy
        // wrapper acquires one permit immediately before each actual HTTP
        // request, so local hits consume no network budget.
        if let Some(client) = self.gameta_client.clone() {
            let get_result = self.with_authorized_gameta_request(|materialization_limit| {
                client.get_metadata_with_limit(&source, &product_id, materialization_limit)
            });
            match get_result {
                Some(Ok(Some(meta_resp))) => {
                    debug!("[get_product_metadata] Got metadata from gameta server");
                    if let Some(json) = serialize_metadata_json(&meta_resp, json_limit) {
                        return Some(json);
                    }
                }
                Some(Ok(None)) => {
                    let fetch_result =
                        self.with_authorized_gameta_request(|materialization_limit| {
                            client.fetch_metadata_with_limit(
                                &source,
                                &product_id,
                                false,
                                materialization_limit,
                            )
                        });
                    match fetch_result {
                        Some(Ok(resp)) => {
                            if let Some(meta) = resp.metadata {
                                debug!("[get_product_metadata] Fetched metadata via gameta server");
                                if let Some(json) = serialize_metadata_json(&meta, json_limit) {
                                    return Some(json);
                                }
                            }
                        }
                        Some(Err(_)) => debug!("[get_product_metadata] Server fetch failed"),
                        None => {}
                    }
                }
                Some(Err(_)) => debug!("[get_product_metadata] Gameta server error"),
                None => {}
            }
        }

        debug!("[get_product_metadata] Metadata not found in any source");
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
        let html_request = self.readable_cache_request(&html_key);
        let Some(html_bytes) = self.data_service.get_data_for_request(&html_request) else {
            return;
        };
        let html_str = String::from_utf8_lossy(&html_bytes);
        let Ok(repaired) = self.parse_dlsite_html_to_metadata(product_id, source, &html_str) else {
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

        if self.check_capability(crate::types::PluginCapability::ArchiveMetadataWrite) {
            if let Err(e) = lib_svc.save_metadata(meta) {
                debug!(
                    "[get_product_metadata] Failed to persist repaired extras: {}",
                    e
                );
            }
        }
        debug!("[get_product_metadata] Repaired cached metadata extras");
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn bounded_metadata_serializer_rejects_before_exceeding_its_output_budget() {
        let value = serde_json::json!({"description": "x".repeat(128)});

        assert!(serialize_metadata_json(&value, 32).is_none());
        let encoded = serialize_metadata_json(&serde_json::json!({"ok": true}), 32)
            .expect("small metadata should serialize");
        assert_eq!(encoded, r#"{"ok":true}"#);
    }

    #[test]
    fn oversized_metadata_summary_batch_is_rejected_before_query() {
        let queries = AtomicUsize::new(0);
        let ids = (0..=MAX_METADATA_SUMMARY_IDS)
            .map(|index| format!("RJ{index:06}"))
            .collect();

        let summaries = metadata_summaries_with_query(ids, |_, _| {
            queries.fetch_add(1, Ordering::SeqCst);
            Ok::<_, anyhow::Error>(Vec::new())
        });

        assert!(summaries.is_empty());
        assert_eq!(queries.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn oversized_metadata_summary_id_is_rejected_before_query() {
        let queries = AtomicUsize::new(0);

        let summaries = metadata_summaries_with_query(
            vec!["x".repeat(MAX_METADATA_PRODUCT_ID_BYTES + 1)],
            |_, _| {
                queries.fetch_add(1, Ordering::SeqCst);
                Ok::<_, anyhow::Error>(Vec::new())
            },
        );

        assert!(summaries.is_empty());
        assert_eq!(queries.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn metadata_summary_output_drops_titles_before_crossing_aggregate_budget() {
        let projected = arclain_core::MetadataSummary {
            id: "dlsite:RJ000001".to_string(),
            title: Some("x".repeat(MAX_METADATA_COLLECTION_BYTES)),
            geo_blocked: false,
        };
        let requested_limit = AtomicUsize::new(0);

        let summaries = metadata_summaries_with_query(vec!["RJ000001".to_string()], |_, limit| {
            requested_limit.store(limit, Ordering::SeqCst);
            Ok::<_, anyhow::Error>(vec![projected])
        });

        assert_eq!(
            requested_limit.load(Ordering::SeqCst),
            MAX_METADATA_SUMMARY_IDS
        );
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "RJ000001");
        assert!(summaries[0].title.is_none());
    }

    #[test]
    fn cached_entry_listing_uses_limited_query_and_aggregate_budget() {
        let requested_limit = AtomicUsize::new(0);
        let oversized = format!("dlsite:{}", "a".repeat(MAX_METADATA_COLLECTION_BYTES + 1));
        let mut retained_bytes = 0;

        assert!(bounded_external_id(&oversized, &mut retained_bytes).is_none());
        assert_eq!(retained_bytes, 0);

        let entries = cached_entries_with_query(|limit| {
            requested_limit.store(limit, Ordering::SeqCst);
            Ok::<_, anyhow::Error>(vec![
                format!("dlsite:{}", "a".repeat(MAX_METADATA_COLLECTION_BYTES)),
                "dlsite:b".to_string(),
            ])
        });

        assert_eq!(requested_limit.load(Ordering::SeqCst), MAX_CACHED_ENTRY_IDS);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].len() <= MAX_METADATA_COLLECTION_BYTES);
    }

    #[test]
    fn cached_metadata_pages_preserve_source_and_apply_offset_and_limit() {
        let requested = std::sync::Mutex::new(None);
        let page = cached_metadata_page_with_query("steam", 4, 2, |source, offset, limit| {
            *requested.lock().unwrap() = Some((source, offset, limit));
            Ok::<_, anyhow::Error>(vec!["steam:100".to_string(), "steam:200".to_string()])
        })
        .expect("valid source page");

        assert_eq!(
            *requested.lock().unwrap(),
            Some((arclain_core::MetadataSource::Steam, 4, 2))
        );
        assert_eq!(page, vec!["100", "200"]);
        assert!(cached_metadata_page_with_query("unknown", 0, 1, |_, _, _| {
            unreachable!("invalid sources must fail before database access")
        })
        .is_err());
        assert!(cached_metadata_page_with_query(
            "steam",
            0,
            (MAX_CACHED_METADATA_PAGE_ITEMS + 1) as u32,
            |_, _, _| unreachable!("oversized pages must fail before database access")
        )
        .is_err());
    }

    #[test]
    fn metadata_summaries_use_the_explicit_source_namespace() {
        let summaries = metadata_summaries_for_source_with_query(
            "steam",
            vec!["42".to_string()],
            |full_ids, _| {
                assert_eq!(full_ids, &["steam:42"]);
                Ok::<_, anyhow::Error>(vec![arclain_core::MetadataSummary {
                    id: "steam:42".to_string(),
                    title: Some("A Steam title".to_string()),
                    geo_blocked: false,
                }])
            },
        )
        .expect("valid explicit source");

        assert_eq!(summaries[0].id, "42");
        assert_eq!(summaries[0].title.as_deref(), Some("A Steam title"));
    }

    #[test]
    fn metadata_write_budget_bounds_rate_distinct_ids_and_session_bytes() {
        let start = std::time::Instant::now();
        let mut rate = MetadataWriteBudget::default();
        for index in 0..MAX_METADATA_WRITES_PER_MINUTE {
            assert!(rate.admit_at(&format!("dlsite:RJ{index:06}"), 1, start));
        }
        assert!(!rate.admit_at("dlsite:RJ999999", 1, start));

        let mut distinct = MetadataWriteBudget::default();
        for index in 0..MAX_METADATA_DISTINCT_IDS_PER_SESSION {
            assert!(distinct.admit_at(
                &format!("dlsite:RJ{index:06}"),
                1,
                start + std::time::Duration::from_secs(index as u64 * 61),
            ));
        }
        assert!(
            !distinct.admit_at(
                "dlsite:one-too-many",
                1,
                start
                    + std::time::Duration::from_secs(
                        MAX_METADATA_DISTINCT_IDS_PER_SESSION as u64 * 61,
                    ),
            )
        );

        let mut bytes = MetadataWriteBudget::default();
        assert!(bytes.admit_at("dlsite:A", MAX_METADATA_BYTES_PER_SESSION, start));
        assert!(!bytes.admit_at("dlsite:A", 1, start + std::time::Duration::from_secs(61),));
    }
}
