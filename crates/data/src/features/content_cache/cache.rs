//! Unified content cache API combining cacache with SQLite index
//!
//! This provides a high-level API for caching binary content (images, etc.)
//! using cacache for content-addressable storage and SQLite for indexing.

use anyhow::{Context, Result};
use arclain_db::{
    get_cache_entry, has_cache_entry, init_cache_index_schema, touch_cache_entry,
    upsert_cache_entry, CacheEntry, CacheType, SqliteDb,
};
use std::path::{Path, PathBuf};

/// Unified content cache combining cacache blob storage with SQLite index
pub struct ContentCache {
    cache_dir: PathBuf,
    index_db: SqliteDb,
}

impl ContentCache {
    /// Create a new content cache at the given base directory.
    ///
    /// Creates the following structure:
    /// ```text
    /// <base_cache_dir>/
    ///   content/
    ///     images/    <- cacache stores content here
    /// ```
    pub fn new(base_cache_dir: PathBuf, index_db: SqliteDb) -> Result<Self> {
        // Create organized folder structure
        let images_dir = base_cache_dir.join("content").join("images");

        std::fs::create_dir_all(&images_dir)
            .with_context(|| format!("Creating cache directory {:?}", images_dir))?;

        // Initialize SQLite schema
        index_db.init_schema(init_cache_index_schema)?;

        Ok(Self {
            cache_dir: images_dir,
            index_db,
        })
    }

    /// Check if content exists in cache
    pub fn has(&self, key: &str) -> Result<bool> {
        self.index_db
            .with_connection(|conn| has_cache_entry(conn, key))
    }

    /// Get content from cache
    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        // First check SQLite index
        let entry = self
            .index_db
            .with_connection(|conn| get_cache_entry(conn, key))?;

        let Some(entry) = entry else {
            return Ok(None);
        };

        // Parse the stored hash string back to Integrity
        let integrity: ssri::Integrity = entry
            .content_hash
            .parse()
            .context("Invalid integrity hash in cache index")?;

        // Read from cacache using the stored hash
        match cacache::read_hash_sync(&self.cache_dir, &integrity) {
            Ok(data) => {
                // Update last accessed time
                let _ = self
                    .index_db
                    .with_connection(|conn| touch_cache_entry(conn, key));
                Ok(Some(data))
            }
            Err(e) => {
                // Cache file missing - index is stale
                tracing::warn!("Cache entry {} has stale index: {}", key, e);
                Ok(None)
            }
        }
    }

    /// Store content in cache
    pub fn put(
        &self,
        key: &str,
        data: &[u8],
        cache_type: CacheType,
        product_id: Option<&str>,
        source_url: Option<&str>,
    ) -> Result<()> {
        // Write to cacache (content-addressable)
        let integrity = cacache::write_hash_sync(&self.cache_dir, data)
            .context("Failed to write to cacache")?;

        let hash_str = integrity.to_string();

        // Update SQLite index
        self.index_db.with_connection(|conn| {
            upsert_cache_entry(
                conn,
                key,
                product_id,
                &hash_str,
                source_url,
                cache_type,
                Some(data.len() as i64),
            )
        })?;

        Ok(())
    }

    /// Get cache entry metadata (without loading content)
    pub fn get_entry(&self, key: &str) -> Result<Option<CacheEntry>> {
        self.index_db
            .with_connection(|conn| get_cache_entry(conn, key))
    }

    /// Get all entries for a product ID
    pub fn get_product_entries(&self, product_id: &str) -> Result<Vec<CacheEntry>> {
        self.index_db
            .with_connection(|conn| arclain_db::get_entries_by_product(conn, product_id))
    }

    /// Generate a cache key for a screenshot
    pub fn screenshot_key(product_id: &str, index: usize) -> String {
        format!("dlsite:{}:screenshot_{}", product_id, index)
    }

    /// Generate a cache key for a thumbnail
    pub fn thumbnail_key(product_id: &str) -> String {
        format!("dlsite:{}:thumbnail", product_id)
    }

    /// Get the cache directory path
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }
}
