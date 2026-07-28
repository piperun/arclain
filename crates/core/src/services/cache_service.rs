use anyhow::Result;
use arclain_db::cache::cache_index;
use arclain_db::{CacheEntry, CacheType, DieselPool};

pub struct CacheService {
    pool: DieselPool,
}

impl CacheService {
    pub fn new(pool: DieselPool) -> Self {
        Self { pool }
    }

    pub fn upsert(
        &self,
        key: &str,
        product_id: Option<&str>,
        content_hash: &str,
        source_url: Option<&str>,
        cache_type: CacheType,
        size_bytes: Option<i64>,
    ) -> Result<i64> {
        self.pool.with_conn(|conn| {
            cache_index::upsert_cache_entry(
                conn,
                key,
                product_id,
                content_hash,
                source_url,
                cache_type,
                size_bytes,
            )
        })
    }

    pub fn get(&self, key: &str) -> Result<Option<CacheEntry>> {
        self.pool
            .with_conn(|conn| cache_index::get_cache_entry(conn, key))
    }

    pub fn has(&self, key: &str) -> Result<bool> {
        self.pool
            .with_conn(|conn| cache_index::has_cache_entry(conn, key))
    }

    pub fn delete(&self, key: &str) -> Result<bool> {
        self.pool
            .with_conn(|conn| cache_index::delete_cache_entry(conn, key))
    }

    pub fn delete_by_pattern(&self, pattern: &str) -> Result<usize> {
        self.pool
            .with_conn(|conn| cache_index::delete_by_pattern(conn, pattern))
    }

    pub fn update_last_accessed(&self, key: &str) -> Result<()> {
        self.pool
            .with_conn(|conn| cache_index::touch_cache_entry(conn, key))
    }

    pub fn entries_lru(&self) -> Result<Vec<CacheEntry>> {
        self.pool.with_conn(cache_index::list_entries_lru)
    }

    pub fn has_content_hash(&self, content_hash: &str) -> Result<bool> {
        self.pool
            .with_conn(|conn| cache_index::has_content_hash(conn, content_hash))
    }

    pub fn entries_lru_page(&self, offset: usize, limit: usize) -> Result<Vec<CacheEntry>> {
        self.pool
            .with_conn(|conn| cache_index::list_entries_lru_page(conn, offset, limit))
    }

    pub fn content_hashes(&self) -> Result<Vec<String>> {
        self.pool.with_conn(cache_index::get_all_content_hashes)
    }

    pub fn clear_all(&self) -> Result<()> {
        self.pool
            .with_conn(|conn| cache_index::clear_all_entries(conn))
    }
}

/// Bridge to `arclain_data::CacheIndex` so `ContentCache` can hold
/// `Arc<dyn CacheIndex>` without depending on this crate.
/// See `arclain_data::traits` for why.
impl arclain_data::CacheIndex for CacheService {
    fn upsert(
        &self,
        key: &str,
        product_id: Option<&str>,
        content_hash: &str,
        source_url: Option<&str>,
        cache_type: CacheType,
        size_bytes: Option<i64>,
    ) -> Result<i64> {
        CacheService::upsert(
            self,
            key,
            product_id,
            content_hash,
            source_url,
            cache_type,
            size_bytes,
        )
    }

    fn get(&self, key: &str) -> Result<Option<CacheEntry>> {
        CacheService::get(self, key)
    }

    fn has(&self, key: &str) -> Result<bool> {
        CacheService::has(self, key)
    }

    fn delete(&self, key: &str) -> Result<bool> {
        CacheService::delete(self, key)
    }

    fn delete_by_pattern(&self, pattern: &str) -> Result<usize> {
        CacheService::delete_by_pattern(self, pattern)
    }

    fn update_last_accessed(&self, key: &str) -> Result<()> {
        CacheService::update_last_accessed(self, key)
    }

    fn entries_lru(&self) -> Result<Vec<CacheEntry>> {
        CacheService::entries_lru(self)
    }

    fn has_content_hash(&self, content_hash: &str) -> Result<bool> {
        CacheService::has_content_hash(self, content_hash)
    }

    fn entries_lru_page(&self, offset: usize, limit: usize) -> Result<Vec<CacheEntry>> {
        CacheService::entries_lru_page(self, offset, limit)
    }

    fn supports_lru_paging(&self) -> bool {
        true
    }

    fn content_hashes(&self) -> Result<Vec<String>> {
        CacheService::content_hashes(self)
    }

    fn has_complete_lru_view(&self) -> bool {
        true
    }
}
