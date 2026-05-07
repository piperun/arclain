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

    pub fn clear_all(&self) -> Result<()> {
        self.pool
            .with_conn(|conn| cache_index::clear_all_entries(conn))
    }
}
