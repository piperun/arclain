//! Legacy Rusqlite implementation for cache index
//!
//! Moved from crates/db/src/cache/cache_index.rs

use crate::cache::cache_index::{CacheEntry, CacheType};
use anyhow::{Context, Result};
use rusqlite::Connection;
use rusqlite::OptionalExtension;

/// Initialize the cache_index table schema
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

/// Insert or update a cache entry
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

/// Get a cache entry by key
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

/// Get all cache entries for a product
pub fn get_entries_by_product(conn: &Connection, product_id: &str) -> Result<Vec<CacheEntry>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, key, product_id, content_hash, source_url, cache_type, 
               created_at, last_accessed, size_bytes
        FROM cache_index WHERE product_id = ?1
        ORDER BY created_at DESC
        "#,
    )?;

    let entries = stmt
        .query_map([product_id], |row| {
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
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entries)
}

/// Check if a cache entry exists
pub fn has_cache_entry(conn: &Connection, key: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM cache_index WHERE key = ?1",
        [key],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Delete a cache entry by key
pub fn delete_cache_entry(conn: &Connection, key: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM cache_index WHERE key = ?1", [key])?;
    Ok(affected > 0)
}

/// Update last_accessed timestamp
pub fn touch_cache_entry(conn: &Connection, key: &str) -> Result<()> {
    conn.execute(
        "UPDATE cache_index SET last_accessed = CURRENT_TIMESTAMP WHERE key = ?1",
        [key],
    )?;
    Ok(())
}

/// Remove all cache entries
pub fn clear_all_entries(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM cache_index", [])
        .context("Failed to clear cache index")?;
    Ok(())
}
