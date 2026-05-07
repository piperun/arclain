//! Rusqlite-backed helpers for the legacy `cache_index` table.
//!
//! The diesel-backed implementation lives in [`crate::cache::cache_index`];
//! this module is the rusqlite mirror used by `CacheDb::open` (which
//! still operates over a raw `rusqlite::Connection` for migration
//! purposes) and the unit tests in `cache/tests.rs`.
//!
//! Only the four helpers actually called from those two sites are kept.
//! The legacy `delete_cache_entry`, `touch_cache_entry`,
//! `clear_all_entries`, and `get_entries_by_product` had no callers and
//! were dropped along with the obsolete `legacy/` module.

#[cfg(test)]
use crate::cache::types::{CacheEntry, CacheType};
use anyhow::{Context, Result};
use rusqlite::Connection;
#[cfg(test)]
use rusqlite::OptionalExtension;

/// Initialize the cache_index table schema.
pub fn init_cache_index_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS cache_index (
            id INTEGER PRIMARY KEY,
            key TEXT UNIQUE NOT NULL,
            product_id TEXT,
            content_hash TEXT NOT NULL,
            source_url TEXT,
            cache_type TEXT NOT NULL DEFAULT 'other',
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            last_accessed TEXT,
            size_bytes INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_cache_product ON cache_index(product_id);
        CREATE INDEX IF NOT EXISTS idx_cache_type ON cache_index(cache_type);
        CREATE INDEX IF NOT EXISTS idx_cache_key ON cache_index(key);
        "#,
    )
    .context("Failed to create cache_index table")?;
    Ok(())
}

/// Insert or update a cache entry.
#[cfg(test)]
pub fn upsert_cache_entry(
    conn: &Connection,
    key: &str,
    product_id: Option<&str>,
    content_hash: &str,
    source_url: Option<&str>,
    cache_type: CacheType,
    size_bytes: Option<i64>,
) -> Result<i64> {
    conn.execute(
        r#"
        INSERT INTO cache_index (key, product_id, content_hash, source_url, cache_type, size_bytes)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(key) DO UPDATE SET
            content_hash = excluded.content_hash,
            source_url = excluded.source_url,
            last_accessed = CURRENT_TIMESTAMP,
            size_bytes = excluded.size_bytes
        "#,
        rusqlite::params![
            key,
            product_id,
            content_hash,
            source_url,
            cache_type.as_str(),
            size_bytes
        ],
    )
    .context("Failed to upsert cache entry")?;

    Ok(conn.last_insert_rowid())
}

/// Get a cache entry by key.
#[cfg(test)]
pub fn get_cache_entry(conn: &Connection, key: &str) -> Result<Option<CacheEntry>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, key, product_id, content_hash, source_url, cache_type,
               created_at, last_accessed, size_bytes
        FROM cache_index WHERE key = ?1
        "#,
    )?;

    let entry = stmt
        .query_row([key], |row| {
            Ok(CacheEntry {
                id: row.get(0)?,
                key: row.get(1)?,
                product_id: row.get(2)?,
                content_hash: row.get(3)?,
                source_url: row.get(4)?,
                cache_type: CacheType::from_str(&row.get::<_, String>(5)?),
                created_at: row.get(6)?,
                last_accessed: row.get(7)?,
                size_bytes: row.get(8)?,
            })
        })
        .optional()?;

    Ok(entry)
}

/// Check if a cache entry exists.
#[cfg(test)]
pub fn has_cache_entry(conn: &Connection, key: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM cache_index WHERE key = ?1",
        [key],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}
