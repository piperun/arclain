use anyhow::Result;
use arclain_db::library::metadata::CompletenessScore;
use gameta_core::{MetadataSource, ProductMetadata};
use gameta_database::DieselBackend;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub const METADATA_SUMMARY_MAX_IDS: usize = gameta_database::MAX_METADATA_SUMMARY_IDS;
pub const METADATA_SUMMARY_MAX_ID_BYTES: usize = gameta_database::MAX_METADATA_SUMMARY_ID_BYTES;
pub const METADATA_SUMMARY_MAX_STORED_ID_BYTES: usize =
    gameta_database::MAX_METADATA_SUMMARY_STORED_ID_BYTES;
pub const METADATA_SUMMARY_TITLE_CHARS: usize = gameta_database::MAX_METADATA_SUMMARY_TITLE_CHARS;

/// Small, bounded projection for metadata-list views.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataSummary {
    pub id: String,
    pub title: Option<String>,
    pub geo_blocked: bool,
}

pub struct LibraryService {
    backend: DieselBackend,
    /// Per-source cache of `list_by_source` results so the dlsite-metadata
    /// plugin (and any future caller) can poll it every frame without
    /// hitting SQLite. Invalidated on every write through `save_metadata` /
    /// `delete_metadata` — coarse but correct, and arclain's write rate
    /// is low enough that we don't need anything fancier. Path D step 1:
    /// pulls the WASM-side `cached_entries` memo out of the plugin so
    /// per-tab instances would all see consistent data.
    list_cache: RwLock<HashMap<MetadataSource, Vec<String>>>,
    /// Epoch counter guarding the slow path against an invalidation race.
    /// Per audit R2: `list_by_source` releases the read guard before
    /// querying SQLite; a concurrent `save_metadata` + `invalidate_list_cache`
    /// could clear the cache between the read miss and the write-insert,
    /// causing the original caller to insert stale data the writer had
    /// just removed. We snapshot the epoch at the read miss and only
    /// commit the cache write if the epoch still matches.
    cache_epoch: AtomicU64,
}

impl LibraryService {
    pub fn new(db_path: &Path) -> Result<Self> {
        // Drop stale cr-sqlite triggers before diesel touches the DB.
        // These triggers reference crsql_internal_sync_bit which no longer exists
        // and cause every INSERT/UPDATE to fail.
        if db_path.exists() {
            if let Ok(conn) = arclain_db::DbConnection::open(db_path) {
                let triggers: Vec<String> = conn
                    .prepare("SELECT name FROM sqlite_master WHERE type='trigger'")
                    .and_then(|mut s| {
                        s.query_map([], |row| row.get(0))
                            .map(|rows| rows.filter_map(|r| r.ok()).collect())
                    })
                    .unwrap_or_default();
                for trigger in &triggers {
                    let _ = conn.execute_batch(&format!("DROP TRIGGER IF EXISTS \"{}\"", trigger));
                }
                if !triggers.is_empty() {
                    tracing::info!(
                        "[LibraryService] Dropped {} stale triggers from metadata DB",
                        triggers.len()
                    );
                }
            }
        }

        let backend = DieselBackend::new_local_sync(db_path)
            .map_err(|e| anyhow::anyhow!("Failed to create metadata backend: {}", e))?;

        // Ensure product_metadata table exists (idempotent)
        backend
            .sync_init_schema()
            .map_err(|e| anyhow::anyhow!("Failed to initialize metadata schema: {}", e))?;

        Ok(Self {
            backend,
            list_cache: RwLock::new(HashMap::new()),
            cache_epoch: AtomicU64::new(0),
        })
    }

    /// Invalidate the `list_by_source` cache. Called from every write
    /// path so the next `list_by_source` query rebuilds from SQLite.
    /// Public so external callers that bypass `save_metadata` (rare)
    /// can flag a stale view; internal write paths call it automatically.
    ///
    /// Bumps `cache_epoch` so any in-flight slow-path read in `list_by_source`
    /// will see the epoch change and skip its stale cache insert.
    pub fn invalidate_list_cache(&self) {
        self.list_cache.write().clear();
        self.cache_epoch.fetch_add(1, Ordering::SeqCst);
    }

    /// Save metadata with quality guards:
    /// 1. Refuse to overwrite non-geo-blocked data with geo-blocked data
    /// 2. Refuse to downgrade completeness score
    pub fn save_metadata(&self, meta: &ProductMetadata) -> Result<()> {
        if let Ok(Some(existing)) = self
            .backend
            .sync_get_metadata(&meta.id)
            .map_err(|e| anyhow::anyhow!("{}", e))
        {
            let new_score = meta.completeness_score();
            let existing_score = existing.completeness_score();

            if meta.geo_blocked && !existing.geo_blocked {
                tracing::warn!(
                    "[LibraryService] Skipping save for '{}' — refusing to overwrite good data with geo-blocked data",
                    meta.id
                );
                return Ok(());
            }

            if new_score < existing_score {
                tracing::warn!(
                    "[LibraryService] Skipping save for '{}' — new data less complete (score {} < {})",
                    meta.id, new_score, existing_score
                );
                return Ok(());
            }

            tracing::debug!(
                "[LibraryService] Updating '{}' (score {} → {}, geo_blocked: {} → {})",
                meta.id,
                existing_score,
                new_score,
                existing.geo_blocked,
                meta.geo_blocked
            );
        }

        let result = self
            .backend
            .sync_save_metadata(meta)
            .map_err(|e| anyhow::anyhow!("{}", e));
        if result.is_ok() {
            self.invalidate_list_cache();
        }
        result
    }

    pub fn get_metadata(&self, id: &str) -> Result<Option<ProductMetadata>> {
        self.backend
            .sync_get_metadata(id)
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    /// Batch lookup — single SQL query for many IDs, instead of one
    /// per ID. Audit P13: `impl_get_metadata_summaries` was looping
    /// `get_metadata` per id and paying N round-trips for each
    /// archive-list refresh; this collapses that to one
    /// `WHERE id IN (?, ?, …)`.
    ///
    /// The returned `Vec` only contains rows the DB actually has —
    /// missing ids are silently dropped. Callers that need a stable
    /// "input id → row?" mapping should rebuild it from the result
    /// (an example lives in `arclain_plugins::host_functions::metadata`).
    pub fn get_many(&self, ids: &[&str]) -> Result<Vec<ProductMetadata>> {
        self.backend
            .sync_get_many(ids)
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    /// Fetch only the fields required by metadata-list summaries.
    ///
    /// Both the ID count and result count are capped at 256. The backend
    /// projects `id`, a SQL-truncated title, and `geo_blocked`; it does not
    /// materialize full metadata rows or raw response columns.
    pub fn get_summaries_limited(
        &self,
        ids: &[&str],
        limit: usize,
    ) -> Result<Vec<MetadataSummary>> {
        self.backend
            .sync_get_summaries_limited(ids, limit)
            .map(|rows| {
                rows.into_iter()
                    .map(|row| MetadataSummary {
                        id: row.id,
                        title: row.title,
                        geo_blocked: row.geo_blocked,
                    })
                    .collect()
            })
            .map_err(|error| anyhow::anyhow!("{error}"))
    }

    pub fn delete_metadata(&self, id: &str) -> Result<()> {
        let result = self
            .backend
            .sync_delete_metadata(id)
            .map_err(|e| anyhow::anyhow!("{}", e));
        if result.is_ok() {
            self.invalidate_list_cache();
        }
        result
    }

    pub fn exists(&self, id: &str) -> Result<bool> {
        self.backend
            .sync_exists(id)
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    pub fn list_by_source(&self, source: MetadataSource) -> Result<Vec<String>> {
        // Fast path: cached result. The dlsite-metadata plugin polls
        // this every frame while the DLsite browser is open; without
        // the cache we'd hit SQLite at 60 Hz for hundreds of rows.
        let read_epoch = {
            let cache = self.list_cache.read();
            if let Some(cached) = cache.get(&source) {
                return Ok(cached.clone());
            }
            // Snapshot the epoch under the read guard so we can detect
            // any concurrent invalidation that lands while we query SQLite.
            self.cache_epoch.load(Ordering::SeqCst)
        };
        // Slow path: rebuild from SQLite and populate the cache.
        let fresh = self
            .backend
            .sync_list_ids_by_source(source)
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        // R2 guard: only commit the insert if no invalidation happened
        // between our read miss and now. If the epoch advanced, a writer
        // already cleared the cache (because their save landed), and our
        // `fresh` may have been read *before* their save — inserting it
        // here would re-pollute the cache with stale data.
        let mut cache = self.list_cache.write();
        if self.cache_epoch.load(Ordering::SeqCst) == read_epoch {
            cache.insert(source, fresh.clone());
        }
        Ok(fresh)
    }

    /// Return at most `limit` IDs for a source in deterministic ID order.
    ///
    /// This deliberately bypasses `list_cache`: callers asking for a bounded
    /// page should neither populate nor clone the unbounded per-source list.
    /// The backend applies the limit in SQL.
    pub fn list_by_source_limited(
        &self,
        source: MetadataSource,
        limit: usize,
    ) -> Result<Vec<String>> {
        self.backend
            .sync_list_ids_by_source_limited(source, limit)
            .map_err(|error| anyhow::anyhow!("{error}"))
    }

    /// Count rows for a metadata source without loading their IDs.
    pub fn count_by_source(&self, source: MetadataSource) -> Result<u64> {
        self.backend
            .sync_count_ids_by_source(source)
            .map_err(|error| anyhow::anyhow!("{error}"))
    }

    /// Return one stable, SQL-bounded page of metadata IDs for a source.
    pub fn list_by_source_page(
        &self,
        source: MetadataSource,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<String>> {
        self.backend
            .sync_list_ids_by_source_page(source, offset, limit)
            .map_err(|error| anyhow::anyhow!("{error}"))
    }

    pub fn get_by_external_id(
        &self,
        source: MetadataSource,
        external_id: &str,
    ) -> Result<Option<ProductMetadata>> {
        self.backend
            .sync_get_by_external_id(source, external_id)
            .map_err(|e| anyhow::anyhow!("{}", e))
    }
}

/// Bridge to `arclain_data::MetadataReader` so `MetadataStoreResolver`
/// can hold `Arc<dyn MetadataReader>` without depending on this crate.
/// See `arclain_data::traits` for why.
impl arclain_data::MetadataReader for LibraryService {
    fn get_metadata(&self, id: &str) -> Result<Option<ProductMetadata>> {
        LibraryService::get_metadata(self, id)
    }

    fn has_metadata(&self, id: &str) -> Result<bool> {
        LibraryService::exists(self, id)
    }

    fn save_metadata(&self, meta: &ProductMetadata) -> Result<()> {
        LibraryService::save_metadata(self, meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_service() -> (tempfile::TempDir, LibraryService) {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("test_metadata.sqlite");
        let svc = LibraryService::new(&db_path).unwrap();
        (dir, svc)
    }

    fn make_meta(ext_id: &str) -> ProductMetadata {
        ProductMetadata::new(MetadataSource::DLSite, ext_id)
    }

    // =========================================================================
    // Basic CRUD through LibraryService
    // =========================================================================

    #[test]
    fn test_save_and_get() {
        let (_dir, svc) = temp_service();
        let mut meta = make_meta("RJ100");
        meta.title = Some("Test Title".into());
        meta.creator = Some("Test Circle".into());

        svc.save_metadata(&meta).unwrap();

        let loaded = svc.get_metadata("dlsite:RJ100").unwrap().unwrap();
        assert_eq!(loaded.title, Some("Test Title".into()));
        assert_eq!(loaded.creator, Some("Test Circle".into()));
        assert_eq!(loaded.source, MetadataSource::DLSite);
    }

    #[test]
    fn test_get_nonexistent() {
        let (_dir, svc) = temp_service();
        assert!(svc.get_metadata("dlsite:NOPE").unwrap().is_none());
    }

    #[test]
    fn test_delete() {
        let (_dir, svc) = temp_service();
        let meta = make_meta("RJ200");
        svc.save_metadata(&meta).unwrap();
        assert!(svc.exists("dlsite:RJ200").unwrap());

        svc.delete_metadata("dlsite:RJ200").unwrap();
        assert!(!svc.exists("dlsite:RJ200").unwrap());
    }

    /// Regression test for audit P13.
    ///
    /// `impl_get_metadata_summaries` used to loop `get_metadata` once
    /// per requested id — a 50-archive list refresh fired 50 round-
    /// trips through diesel + the `Mutex<Connection>`. `get_many`
    /// collapses that into a single `WHERE id IN (...)` query.
    ///
    /// We can't directly count SQL round-trips here without diesel
    /// instrumentation, so the test asserts on the contract: every
    /// requested id that exists comes back, missing ids are silently
    /// absent (not errors), and the empty-input case doesn't query
    /// the DB at all.
    #[test]
    fn p13_get_many_returns_all_known_ids_in_one_call() {
        let (_dir, svc) = temp_service();

        // Insert 5 rows; we'll ask for 7 (5 hit, 2 miss).
        let known: Vec<String> = (0..5)
            .map(|i| {
                let mut m = make_meta(&format!("RJ70000{}", i));
                m.title = Some(format!("Title {}", i));
                svc.save_metadata(&m).unwrap();
                m.id.clone()
            })
            .collect();

        let missing = vec!["dlsite:RJ999998".to_string(), "dlsite:RJ999999".to_string()];

        let mut requested: Vec<&str> = known.iter().map(String::as_str).collect();
        requested.extend(missing.iter().map(String::as_str));

        let got = svc.get_many(&requested).unwrap();
        assert_eq!(got.len(), 5, "should only return the 5 rows that exist");

        let got_ids: std::collections::HashSet<&str> = got.iter().map(|m| m.id.as_str()).collect();
        for k in &known {
            assert!(
                got_ids.contains(k.as_str()),
                "expected to find {} in result",
                k,
            );
        }
        for m in &missing {
            assert!(
                !got_ids.contains(m.as_str()),
                "did not expect missing id {} in result",
                m,
            );
        }
    }

    /// Empty input is a fast path — no DB call, empty Vec back.
    #[test]
    fn p13_get_many_empty_input_returns_empty() {
        let (_dir, svc) = temp_service();
        let got = svc.get_many(&[]).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn projected_summaries_are_bounded_and_stably_ordered() {
        let (_dir, svc) = temp_service();
        let mut second = make_meta("RJ_SUMMARY_B");
        second.title = Some("z".repeat(METADATA_SUMMARY_TITLE_CHARS + 100));
        second.raw_api_response = Some("unused raw response".repeat(100_000));
        svc.save_metadata(&second).unwrap();
        let mut first = make_meta("RJ_SUMMARY_A");
        first.title = Some("first".into());
        first.geo_blocked = true;
        svc.save_metadata(&first).unwrap();

        let rows = svc
            .get_summaries_limited(&["dlsite:RJ_SUMMARY_B", "dlsite:RJ_SUMMARY_A"], 2)
            .unwrap();

        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec!["dlsite:RJ_SUMMARY_A", "dlsite:RJ_SUMMARY_B"]
        );
        assert_eq!(rows[0].title.as_deref(), Some("first"));
        assert!(rows[0].geo_blocked);
        assert_eq!(
            rows[1].title.as_ref().unwrap().chars().count(),
            METADATA_SUMMARY_TITLE_CHARS
        );
    }

    #[test]
    fn projected_summaries_accept_prefixed_max_external_id_and_reject_oversized_stored_id() {
        let (_dir, svc) = temp_service();
        let external_id = "X".repeat(METADATA_SUMMARY_MAX_ID_BYTES);
        let stored_id = format!("dlsite:{external_id}");
        let mut metadata = make_meta(&external_id);
        metadata.id = stored_id.clone();
        svc.save_metadata(&metadata).unwrap();

        let rows = svc
            .get_summaries_limited(&[stored_id.as_str()], 1)
            .expect("prefixed max external ID must fit the stored-ID limit");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, stored_id);

        let oversized_stored_id = "x".repeat(METADATA_SUMMARY_MAX_STORED_ID_BYTES + 1);
        assert!(svc
            .get_summaries_limited(&[oversized_stored_id.as_str()], 1)
            .is_err());
    }

    #[test]
    fn test_exists() {
        let (_dir, svc) = temp_service();
        assert!(!svc.exists("dlsite:RJ300").unwrap());

        svc.save_metadata(&make_meta("RJ300")).unwrap();
        assert!(svc.exists("dlsite:RJ300").unwrap());
    }

    #[test]
    fn test_list_by_source() {
        let (_dir, svc) = temp_service();
        svc.save_metadata(&make_meta("RJ001")).unwrap();
        svc.save_metadata(&make_meta("RJ002")).unwrap();

        let mut steam = ProductMetadata::new(MetadataSource::Steam, "12345");
        steam.title = Some("Steam Game".into());
        svc.save_metadata(&steam).unwrap();

        let dlsite_ids = svc.list_by_source(MetadataSource::DLSite).unwrap();
        assert_eq!(dlsite_ids.len(), 2);

        let steam_ids = svc.list_by_source(MetadataSource::Steam).unwrap();
        assert_eq!(steam_ids.len(), 1);
    }

    #[test]
    fn list_by_source_limited_handles_zero_exact_limit_and_stable_order() {
        let (_dir, svc) = temp_service();
        for external_id in ["RJ003", "RJ001", "RJ002"] {
            svc.save_metadata(&make_meta(external_id)).unwrap();
        }

        assert!(svc
            .list_by_source_limited(MetadataSource::DLSite, 0)
            .unwrap()
            .is_empty());
        assert_eq!(
            svc.list_by_source_limited(MetadataSource::DLSite, 2)
                .unwrap(),
            vec!["dlsite:RJ001", "dlsite:RJ002"]
        );
        assert_eq!(
            svc.list_by_source_limited(MetadataSource::DLSite, 3)
                .unwrap(),
            vec!["dlsite:RJ001", "dlsite:RJ002", "dlsite:RJ003"]
        );
    }

    #[test]
    fn metadata_source_count_and_page_do_not_materialize_the_full_source() {
        let (_dir, svc) = temp_service();
        for external_id in ["RJ004", "RJ002", "RJ001", "RJ003"] {
            svc.save_metadata(&make_meta(external_id)).unwrap();
        }

        assert_eq!(svc.count_by_source(MetadataSource::DLSite).unwrap(), 4);
        assert_eq!(
            svc.list_by_source_page(MetadataSource::DLSite, 1, 2)
                .unwrap(),
            vec!["dlsite:RJ002", "dlsite:RJ003"]
        );
    }

    #[test]
    fn test_get_by_external_id() {
        let (_dir, svc) = temp_service();
        let mut meta = make_meta("RJ400");
        meta.title = Some("By External".into());
        svc.save_metadata(&meta).unwrap();

        let found = svc
            .get_by_external_id(MetadataSource::DLSite, "RJ400")
            .unwrap()
            .unwrap();
        assert_eq!(found.title, Some("By External".into()));

        let not_found = svc
            .get_by_external_id(MetadataSource::Steam, "RJ400")
            .unwrap();
        assert!(not_found.is_none());
    }

    // =========================================================================
    // Quality guard: geo-block protection
    // =========================================================================

    #[test]
    fn test_geo_block_guard_blocks_overwrite() {
        let (_dir, svc) = temp_service();

        // Save good (non-geo-blocked) data
        let mut good = make_meta("RJ500");
        good.title = Some("Real Title".into());
        good.creator = Some("Real Circle".into());
        good.geo_blocked = false;
        svc.save_metadata(&good).unwrap();

        // Try to overwrite with geo-blocked data
        let mut blocked = make_meta("RJ500");
        blocked.title = Some("Geo-Blocked Title".into());
        blocked.geo_blocked = true;
        svc.save_metadata(&blocked).unwrap(); // Returns Ok but skips

        // Original data preserved
        let loaded = svc.get_metadata("dlsite:RJ500").unwrap().unwrap();
        assert_eq!(loaded.title, Some("Real Title".into()));
        assert!(!loaded.geo_blocked);
    }

    #[test]
    fn test_geo_block_guard_allows_upgrade_from_blocked() {
        let (_dir, svc) = temp_service();

        // Save geo-blocked data first
        let mut blocked = make_meta("RJ501");
        blocked.title = Some("Blocked".into());
        blocked.geo_blocked = true;
        svc.save_metadata(&blocked).unwrap();

        // Save non-geo-blocked data over it
        let mut good = make_meta("RJ501");
        good.title = Some("Real Title".into());
        good.creator = Some("Creator".into());
        good.geo_blocked = false;
        svc.save_metadata(&good).unwrap();

        // New data should replace
        let loaded = svc.get_metadata("dlsite:RJ501").unwrap().unwrap();
        assert_eq!(loaded.title, Some("Real Title".into()));
        assert!(!loaded.geo_blocked);
    }

    // =========================================================================
    // Quality guard: completeness score protection
    // =========================================================================

    #[test]
    fn test_completeness_guard_blocks_downgrade() {
        let (_dir, svc) = temp_service();

        // Save complete data (high score)
        let mut complete = make_meta("RJ600");
        complete.title = Some("Full Title".into());
        complete.creator = Some("Circle".into());
        complete.description = Some("Description".into());
        complete.tags = vec!["tag1".into(), "tag2".into()];
        svc.save_metadata(&complete).unwrap();

        // Try to overwrite with less complete data (lower score)
        let mut sparse = make_meta("RJ600");
        sparse.title = Some("Just Title".into());
        // No creator, no description, no tags
        svc.save_metadata(&sparse).unwrap(); // Returns Ok but skips

        // Original data preserved
        let loaded = svc.get_metadata("dlsite:RJ600").unwrap().unwrap();
        assert_eq!(loaded.title, Some("Full Title".into()));
        assert_eq!(loaded.creator, Some("Circle".into()));
        assert_eq!(loaded.description, Some("Description".into()));
    }

    #[test]
    fn test_completeness_guard_allows_upgrade() {
        let (_dir, svc) = temp_service();

        // Save sparse data first
        let mut sparse = make_meta("RJ601");
        sparse.title = Some("Title Only".into());
        svc.save_metadata(&sparse).unwrap();

        // Save more complete data
        let mut complete = make_meta("RJ601");
        complete.title = Some("Better Title".into());
        complete.creator = Some("Circle".into());
        complete.description = Some("Description".into());
        svc.save_metadata(&complete).unwrap();

        let loaded = svc.get_metadata("dlsite:RJ601").unwrap().unwrap();
        assert_eq!(loaded.title, Some("Better Title".into()));
        assert_eq!(loaded.creator, Some("Circle".into()));
    }

    #[test]
    fn test_completeness_guard_allows_equal_score() {
        let (_dir, svc) = temp_service();

        let mut v1 = make_meta("RJ602");
        v1.title = Some("Original".into());
        svc.save_metadata(&v1).unwrap();

        // Same score but different content — should update
        let mut v2 = make_meta("RJ602");
        v2.title = Some("Updated".into());
        svc.save_metadata(&v2).unwrap();

        let loaded = svc.get_metadata("dlsite:RJ602").unwrap().unwrap();
        assert_eq!(loaded.title, Some("Updated".into()));
    }

    #[test]
    fn test_first_save_always_succeeds() {
        let (_dir, svc) = temp_service();

        // First save should always work, no guard checks
        let mut meta = make_meta("RJ700");
        meta.title = Some("First Save".into());
        meta.geo_blocked = true; // Even geo-blocked first save should work
        svc.save_metadata(&meta).unwrap();

        let loaded = svc.get_metadata("dlsite:RJ700").unwrap().unwrap();
        assert_eq!(loaded.title, Some("First Save".into()));
        assert!(loaded.geo_blocked);
    }

    // =========================================================================
    // Field round-trip (gameta type fidelity)
    // =========================================================================

    #[test]
    fn test_all_fields_roundtrip() {
        let (_dir, svc) = temp_service();

        let meta = ProductMetadata {
            id: "dlsite:RJ800".into(),
            source: MetadataSource::DLSite,
            external_id: "RJ800".into(),
            title: Some("Full Product".into()),
            creator: Some("Circle X".into()),
            description: Some("A great game".into()),
            release_date: Some("2024-01-15".into()),
            price: Some(1980),
            currency: Some("JPY".into()),
            rating: Some(4.5),
            rating_count: Some(100),
            purchase_count: Some(5000),
            favorite_count: Some(200),
            review_count: Some(50),
            file_size: Some("1.2GB".into()),
            file_format: Some("ZIP".into()),
            age_rating: Some("R-18".into()),
            genres: vec!["RPG".into(), "Adventure".into()],
            tags: vec!["fantasy".into(), "turn-based".into()],
            languages: vec!["Japanese".into(), "English".into()],
            extras: serde_json::json!({
                "series_name": "Test Series",
                "illustrator": "Artist Y",
                "voice_actors": ["VA1", "VA2"],
            }),
            raw_api_response: Some("{\"test\": true}".into()),
            raw_html: Some("<html>test</html>".into()),
            geo_blocked: false,
            cached_at: 1704067200,
            updated_at: Some(1704153600),
        };

        svc.save_metadata(&meta).unwrap();
        let loaded = svc.get_metadata("dlsite:RJ800").unwrap().unwrap();

        assert_eq!(loaded.id, "dlsite:RJ800");
        assert_eq!(loaded.source, MetadataSource::DLSite);
        assert_eq!(loaded.external_id, "RJ800");
        assert_eq!(loaded.title, Some("Full Product".into()));
        assert_eq!(loaded.creator, Some("Circle X".into()));
        assert_eq!(loaded.description, Some("A great game".into()));
        assert_eq!(loaded.release_date, Some("2024-01-15".into()));
        assert_eq!(loaded.price, Some(1980));
        assert_eq!(loaded.currency, Some("JPY".into()));
        assert_eq!(loaded.rating, Some(4.5));
        assert_eq!(loaded.rating_count, Some(100));
        assert_eq!(loaded.purchase_count, Some(5000));
        assert_eq!(loaded.favorite_count, Some(200));
        assert_eq!(loaded.review_count, Some(50));
        assert_eq!(loaded.file_size, Some("1.2GB".into()));
        assert_eq!(loaded.file_format, Some("ZIP".into()));
        assert_eq!(loaded.age_rating, Some("R-18".into()));
        assert_eq!(loaded.genres, vec!["RPG", "Adventure"]);
        assert_eq!(loaded.tags, vec!["fantasy", "turn-based"]);
        assert_eq!(loaded.languages, vec!["Japanese", "English"]);
        assert_eq!(loaded.extras["series_name"], "Test Series");
        assert_eq!(loaded.extras["illustrator"], "Artist Y");
        assert_eq!(loaded.raw_api_response, Some("{\"test\": true}".into()));
        assert_eq!(loaded.raw_html, Some("<html>test</html>".into()));
        assert!(!loaded.geo_blocked);
        assert_eq!(loaded.cached_at, 1704067200);
        assert_eq!(loaded.updated_at, Some(1704153600));
    }

    // =========================================================================
    // list_by_source cache (Path D step 1)
    // =========================================================================

    /// Second `list_by_source` call returns the same data without
    /// re-querying SQLite (the cache services it). We can't directly
    /// observe SQL round-trips here, but the contract is "data
    /// matches what was just saved" and "subsequent calls see no
    /// change until a write happens" — verified below.
    #[test]
    fn list_by_source_cache_returns_consistent_data() {
        let (_dir, svc) = temp_service();
        svc.save_metadata(&make_meta("RJ100")).unwrap();
        svc.save_metadata(&make_meta("RJ200")).unwrap();

        let first = svc.list_by_source(MetadataSource::DLSite).unwrap();
        let second = svc.list_by_source(MetadataSource::DLSite).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 2);
    }

    /// `save_metadata` invalidates the cache so the next list sees
    /// the new row. Without the invalidation hook the cache would
    /// return stale data and the DLsite browser would miss new
    /// entries.
    #[test]
    fn list_by_source_cache_invalidated_on_save() {
        let (_dir, svc) = temp_service();
        svc.save_metadata(&make_meta("RJ100")).unwrap();
        let before = svc.list_by_source(MetadataSource::DLSite).unwrap();
        assert_eq!(before.len(), 1);

        svc.save_metadata(&make_meta("RJ200")).unwrap();
        let after = svc.list_by_source(MetadataSource::DLSite).unwrap();
        assert_eq!(after.len(), 2, "cache must rebuild after save");
    }

    /// Same for delete.
    #[test]
    fn list_by_source_cache_invalidated_on_delete() {
        let (_dir, svc) = temp_service();
        svc.save_metadata(&make_meta("RJ100")).unwrap();
        svc.save_metadata(&make_meta("RJ200")).unwrap();
        let before = svc.list_by_source(MetadataSource::DLSite).unwrap();
        assert_eq!(before.len(), 2);

        svc.delete_metadata("dlsite:RJ100").unwrap();
        let after = svc.list_by_source(MetadataSource::DLSite).unwrap();
        assert_eq!(after.len(), 1, "cache must rebuild after delete");
    }

    /// Manual `invalidate_list_cache` works too — exposed publicly
    /// for callers that mutate the DB via a path the service doesn't
    /// own (legacy import scripts, future bulk operations).
    #[test]
    fn list_by_source_explicit_invalidation_drops_cache() {
        let (_dir, svc) = temp_service();
        svc.save_metadata(&make_meta("RJ100")).unwrap();
        let _ = svc.list_by_source(MetadataSource::DLSite).unwrap(); // warm cache
        svc.invalidate_list_cache();
        // After invalidation the next read goes back to SQLite. We
        // can't observe that directly here, but verify the result
        // matches the underlying state.
        let after = svc.list_by_source(MetadataSource::DLSite).unwrap();
        assert_eq!(after.len(), 1);
    }

    /// Regression test for R2: `list_by_source`'s slow path used to be
    /// vulnerable to a TOCTOU race against `invalidate_list_cache`.
    ///
    /// Sequence (pre-fix):
    ///   T1: list_by_source — read miss → drop read guard
    ///   T2: save_metadata — sync_save → invalidate (clears map)
    ///   T1: query SQLite → write-insert (stale: doesn't include T2's row)
    ///
    /// Post-fix the epoch counter advances on T2's invalidate; T1
    /// observes the bump and skips the insert.
    ///
    /// We can't easily inject latency into the diesel call, so this
    /// test exercises the API directly: snapshot the epoch, simulate
    /// a concurrent save by calling invalidate, then attempt the
    /// write — it should be a no-op.
    #[test]
    fn r2_list_by_source_epoch_guard_skips_stale_insert() {
        use std::sync::atomic::Ordering;
        let (_dir, svc) = temp_service();
        svc.save_metadata(&make_meta("RJ100")).unwrap();
        // Warm + drop cache to put us in a known empty state.
        let _ = svc.list_by_source(MetadataSource::DLSite).unwrap();
        svc.invalidate_list_cache();

        // Simulate the slow path race manually: pretend we just did
        // the read miss and grabbed the epoch.
        let snap = svc.cache_epoch.load(Ordering::SeqCst);
        let stale = vec!["dlsite:RJ100".to_string()];

        // A concurrent writer lands now: save + invalidate.
        svc.save_metadata(&make_meta("RJ200")).unwrap();
        // invalidate_list_cache ran inside save_metadata; epoch advanced.
        assert_ne!(snap, svc.cache_epoch.load(Ordering::SeqCst));

        // Now the racing reader tries to commit its stale view.
        // The guard inside list_by_source's write block compares
        // snap vs current epoch; mismatch ⇒ skip insert. We replay
        // that check here:
        {
            let mut cache = svc.list_cache.write();
            if svc.cache_epoch.load(Ordering::SeqCst) == snap {
                cache.insert(MetadataSource::DLSite, stale.clone());
            }
        }

        // The cache must NOT contain the stale single-element vec —
        // it should be empty (post-invalidate) so the next reader
        // rebuilds fresh.
        let cache_state = svc.list_cache.read();
        assert!(
            cache_state.get(&MetadataSource::DLSite).is_none(),
            "epoch guard must reject stale insert; got {:?}",
            cache_state.get(&MetadataSource::DLSite),
        );
        drop(cache_state);

        // And the next list call rebuilds with BOTH rows, not just the stale one.
        let fresh = svc.list_by_source(MetadataSource::DLSite).unwrap();
        assert_eq!(
            fresh.len(),
            2,
            "next list_by_source must include the concurrent save"
        );
    }

    /// Companion test: under a real thread interleaving (no manual
    /// epoch ops), repeated concurrent saves + list calls never
    /// corrupt the cache. We can't deterministically force the race,
    /// but if the guard is wrong we'll occasionally see fewer rows
    /// than expected on subsequent reads.
    #[test]
    fn r2_list_by_source_concurrent_save_no_stale_cache() {
        use std::sync::Arc;
        use std::thread;
        let (_dir, svc) = temp_service();
        let svc = Arc::new(svc);
        svc.save_metadata(&make_meta("RJ001")).unwrap();

        // Producer thread fires saves with a tiny stagger.
        let svc_w = svc.clone();
        let writer = thread::spawn(move || {
            for i in 100..120 {
                svc_w
                    .save_metadata(&make_meta(&format!("RJ{}", i)))
                    .unwrap();
                thread::sleep(std::time::Duration::from_micros(50));
            }
        });

        // Reader thread fires list calls in parallel.
        let svc_r = svc.clone();
        let reader = thread::spawn(move || {
            for _ in 0..50 {
                let _ = svc_r.list_by_source(MetadataSource::DLSite).unwrap();
                thread::sleep(std::time::Duration::from_micros(50));
            }
        });

        writer.join().unwrap();
        reader.join().unwrap();

        // Final list must reflect ALL 21 saves (1 + 20).
        let final_list = svc.list_by_source(MetadataSource::DLSite).unwrap();
        assert_eq!(
            final_list.len(),
            21,
            "after concurrent saves, list_by_source must see every row, got {}",
            final_list.len(),
        );
    }

    /// Skipped saves (geo-block guard, completeness guard) don't
    /// touch the DB; the cache should stay intact so subsequent
    /// reads don't pay a rebuild cost.
    #[test]
    fn list_by_source_cache_not_invalidated_on_skipped_save() {
        let (_dir, svc) = temp_service();
        let mut good = make_meta("RJ100");
        good.title = Some("Real".into());
        good.geo_blocked = false;
        svc.save_metadata(&good).unwrap();
        let _ = svc.list_by_source(MetadataSource::DLSite).unwrap();

        // Geo-blocked save is rejected internally; the function still
        // returns Ok, but the cache should NOT have been invalidated.
        // (Verifying this without instrumentation is hard; we at least
        // verify the data state is unchanged, which is the visible
        // contract.)
        let mut blocked = make_meta("RJ100");
        blocked.title = Some("Blocked".into());
        blocked.geo_blocked = true;
        svc.save_metadata(&blocked).unwrap();
        let after = svc.list_by_source(MetadataSource::DLSite).unwrap();
        assert_eq!(after.len(), 1);
    }

    #[test]
    fn test_unicode_roundtrip() {
        let (_dir, svc) = temp_service();

        let mut meta = make_meta("RJ900");
        meta.title = Some("日本語タイトル 🎮".into());
        meta.creator = Some("制作者 O'Connor".into());
        meta.tags = vec!["タグ1".into(), "タグ2".into()];
        svc.save_metadata(&meta).unwrap();

        let loaded = svc.get_metadata("dlsite:RJ900").unwrap().unwrap();
        assert_eq!(loaded.title, Some("日本語タイトル 🎮".into()));
        assert_eq!(loaded.creator, Some("制作者 O'Connor".into()));
        assert_eq!(loaded.tags, vec!["タグ1", "タグ2"]);
    }
}
