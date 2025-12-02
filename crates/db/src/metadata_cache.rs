use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct CachedMetadata {
    pub product_id: String,
    pub title: String,
    pub circle: Option<String>,
    pub price: Option<i64>,
    pub release_date: Option<String>,
    pub metadata_json: String,
    pub cached_at: u64,
}

pub struct MetadataCache {
    conn: Arc<Mutex<Connection>>,
}

impl MetadataCache {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Get cached metadata by product ID
    pub fn get(&self, product_id: &str) -> Result<Option<CachedMetadata>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT product_id, title, circle, price, release_date, metadata_json, cached_at 
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
                    metadata_json: row.get(5)?,
                    cached_at: row.get(6)?,
                })
            })
            .optional()?;

        Ok(entry)
    }

    /// Save metadata to cache
    pub fn save(&self, meta: &CachedMetadata) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO dlsite_metadata_cache 
             (product_id, title, circle, price, release_date, metadata_json, cached_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(product_id) DO UPDATE SET
             title=excluded.title, circle=excluded.circle, price=excluded.price,
             release_date=excluded.release_date, metadata_json=excluded.metadata_json,
             cached_at=excluded.cached_at",
            params![
                meta.product_id,
                meta.title,
                meta.circle,
                meta.price,
                meta.release_date,
                meta.metadata_json,
                meta.cached_at
            ],
        )?;
        Ok(())
    }

    /// Check if cache exists and is fresh (< max_age_days)
    pub fn is_fresh(&self, product_id: &str, max_age_days: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
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
    }

    /// Clear old cache entries
    pub fn clear_old(&self, max_age_days: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let max_age_seconds = (max_age_days as u64) * 24 * 60 * 60;
        let cutoff = now.saturating_sub(max_age_seconds);

        let count = conn.execute(
            "DELETE FROM dlsite_metadata_cache WHERE cached_at < ?1",
            [cutoff],
        )?;
        Ok(count)
    }
}
