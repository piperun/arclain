use crate::sqlite_db::SqliteDb;
use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct CachedMetadata {
    pub product_id: String,
    pub title: String,
    pub circle: Option<String>,
    pub price: Option<i64>,
    pub release_date: Option<String>,
    pub description: Option<String>,
    pub work_type: Option<String>,
    pub file_format: Option<String>,
    pub tags_json: Option<String>, // JSON array of tags
    pub raw_api_json: String,      // Original DLSite API response
    pub cached_at: u64,
}

pub struct MetadataCache {
    db: SqliteDb,
}

impl Clone for MetadataCache {
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
        }
    }
}

impl MetadataCache {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }

    /// Get cached metadata by product ID
    pub fn get(&self, product_id: &str) -> Result<Option<CachedMetadata>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT product_id, title, circle, price, release_date, description, work_type, file_format, tags_json, raw_api_json, cached_at 
                 FROM dlsite_metadata_cache WHERE product_id = ?1",
            )?;

            let entry = stmt
                .query_row([product_id], |row| {
                    Ok(CachedMetadata {
                        product_id: row.get(0)?,
                        title: row.get(1)?,
                        circle: row.get(2)?,
                        price: row.get(3)?,
                        release_date: row.get(4)?,
                        description: row.get(5)?,
                        work_type: row.get(6)?,
                        file_format: row.get(7)?,
                        tags_json: row.get(8)?,
                        raw_api_json: row.get(9)?,
                        cached_at: row.get(10)?,
                    })
                })
                .optional()?;

            Ok(entry)
        })
    }

    /// Save cached metadata
    pub fn save(&self, metadata: &CachedMetadata) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO dlsite_metadata_cache 
                 (product_id, title, circle, price, release_date, description, work_type, file_format, tags_json, raw_api_json, cached_at) 
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    &metadata.product_id,
                    &metadata.title,
                    &metadata.circle,
                    &metadata.price,
                    &metadata.release_date,
                    &metadata.description,
                    &metadata.work_type,
                    &metadata.file_format,
                    &metadata.tags_json,
                    &metadata.raw_api_json,
                    &metadata.cached_at,
                ],
            )?;
            Ok(())
        })
    }

    /// Check if cache exists and is fresh (< max_age_days)
    pub fn is_fresh(&self, product_id: &str, max_age_days: i64) -> Result<bool> {
        self.db.with_connection(|conn| {
            let cached_at: Option<u64> = conn
                .query_row(
                    "SELECT cached_at FROM dlsite_metadata_cache WHERE product_id = ?1",
                    [product_id],
                    |row| row.get(0),
                )
                .optional()?;

            if let Some(timestamp) = cached_at {
                let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
                let age_seconds = now.saturating_sub(timestamp);
                let max_age_seconds = (max_age_days as u64) * 24 * 60 * 60;
                Ok(age_seconds < max_age_seconds)
            } else {
                Ok(false)
            }
        })
    }

    /// Clear all cache entries older than the specified age in days
    pub fn clear_old(&self, max_age_days: i64) -> Result<usize> {
        self.db.with_connection(|conn| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            let max_age_seconds = (max_age_days as u64) * 24 * 60 * 60;
            let cutoff = now.saturating_sub(max_age_seconds);

            let count = conn.execute(
                "DELETE FROM dlsite_metadata_cache WHERE cached_at < ?1",
                [cutoff],
            )?;
            Ok(count)
        })
    }
    /// Clear all cache entries
    pub fn clear_cache_index(&self) -> Result<()> {
        self.db
            .with_connection(|conn| crate::cache_index::clear_all_entries(conn))
    }
}
