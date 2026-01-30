//! Content integrity tracking
//!
//! Tracks SRI (Subresource Integrity) hashes for cached content to ensure
//! data integrity and enable verification of cached files.

use gameta_core::StorageError;
use libsql::params;

use super::helpers::chrono_lite_now;
use super::LibSqlBackend;

impl LibSqlBackend {
    /// Save a content reference with integrity information
    ///
    /// Records the SRI hash and metadata for a cached content item,
    /// enabling later verification that the cached data hasn't been corrupted.
    ///
    /// # Arguments
    /// * `product_id` - The product this content belongs to
    /// * `content_type` - Type of content (e.g., "cover", "screenshot", "thumbnail")
    /// * `cache_key` - The cacache key for retrieving the content
    /// * `sri_hash` - SRI hash (e.g., "sha512-abc123...")
    /// * `source_url` - Original URL the content was fetched from
    /// * `size_bytes` - Size of the content in bytes
    pub async fn save_content_ref(
        &self,
        product_id: &str,
        content_type: &str,
        cache_key: &str,
        sri_hash: &str,
        source_url: Option<&str>,
        size_bytes: Option<i64>,
    ) -> Result<(), StorageError> {
        let now = chrono_lite_now();
        self.conn
            .execute(
                "INSERT OR REPLACE INTO content_refs
                 (product_id, content_type, cache_key, sri_hash, source_url, size_bytes, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![product_id, content_type, cache_key, sri_hash, source_url, size_bytes, now],
            )
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;
        Ok(())
    }

    /// Get the SRI hash for a cache key
    ///
    /// Used to verify the integrity of cached content before serving it.
    pub async fn get_content_hash(&self, cache_key: &str) -> Result<Option<String>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT sri_hash FROM content_refs WHERE cache_key = ?1",
                params![cache_key],
            )
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?
        {
            Ok(row.get(0).ok())
        } else {
            Ok(None)
        }
    }

    /// Mark content as verified
    ///
    /// Updates the `verified_at` timestamp to indicate the content
    /// has been checked and matches its expected hash.
    pub async fn mark_verified(&self, cache_key: &str) -> Result<(), StorageError> {
        let now = chrono_lite_now();
        self.conn
            .execute(
                "UPDATE content_refs SET verified_at = ?1 WHERE cache_key = ?2",
                params![now, cache_key],
            )
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;
        Ok(())
    }

    /// Get all unverified content references
    ///
    /// Returns cache keys for content that hasn't been verified recently.
    /// Useful for background integrity checking.
    pub async fn get_unverified_content(&self) -> Result<Vec<String>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT cache_key FROM content_refs WHERE verified_at IS NULL",
                (),
            )
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;

        let mut keys = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?
        {
            if let Ok(key) = row.get::<String>(0) {
                keys.push(key);
            }
        }
        Ok(keys)
    }
}
