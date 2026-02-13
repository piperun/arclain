use anyhow::Result;
use arclain_db::library::metadata::CompletenessScore;
use gameta_core::{MetadataSource, ProductMetadata};
use gameta_database::DieselBackend;
use std::path::Path;

pub struct LibraryService {
    backend: DieselBackend,
}

impl LibraryService {
    pub fn new(db_path: &Path) -> Result<Self> {
        let backend = DieselBackend::new_local_sync(db_path)
            .map_err(|e| anyhow::anyhow!("Failed to create metadata backend: {}", e))?;

        // Ensure product_metadata table exists (idempotent)
        backend
            .sync_init_schema()
            .map_err(|e| anyhow::anyhow!("Failed to initialize metadata schema: {}", e))?;

        Ok(Self { backend })
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
                meta.id, existing_score, new_score, existing.geo_blocked, meta.geo_blocked
            );
        }

        self.backend
            .sync_save_metadata(meta)
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    pub fn get_metadata(&self, id: &str) -> Result<Option<ProductMetadata>> {
        self.backend
            .sync_get_metadata(id)
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    pub fn delete_metadata(&self, id: &str) -> Result<()> {
        self.backend
            .sync_delete_metadata(id)
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    pub fn exists(&self, id: &str) -> Result<bool> {
        self.backend
            .sync_exists(id)
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    pub fn list_by_source(&self, source: MetadataSource) -> Result<Vec<String>> {
        self.backend
            .sync_list_ids_by_source(source)
            .map_err(|e| anyhow::anyhow!("{}", e))
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
