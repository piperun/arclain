//! Cache index for tracking cached content locations in SQLite
//!
//! This module provides a SQLite-backed index that tracks where cached
//! content is stored in the cacache content-addressable store.

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Type of cached content
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheType {
    Screenshot,
    Thumbnail,
    Metadata,
    Other,
}

impl CacheType {
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheType::Screenshot => "screenshot",
            CacheType::Thumbnail => "thumbnail",
            CacheType::Metadata => "metadata",
            CacheType::Other => "other",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "screenshot" => CacheType::Screenshot,
            "thumbnail" => CacheType::Thumbnail,
            "metadata" => CacheType::Metadata,
            _ => CacheType::Other,
        }
    }
}

/// A cached content entry
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub id: i64,
    pub key: String,
    pub product_id: Option<String>,
    pub content_hash: String,
    pub source_url: Option<String>,
    pub cache_type: CacheType,
    pub created_at: String,
    pub last_accessed: Option<String>,
    pub size_bytes: Option<i64>,
}

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

use rusqlite::OptionalExtension;

/// Remove all cache entries
pub fn clear_all_entries(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM cache_index", [])
        .context("Failed to clear cache index")?;
    Ok(())
}

// ============================================================================
// Diesel DSL versions
// ============================================================================

use diesel::prelude::*;

/// Delete cache entry by key using Diesel DSL
pub fn delete_cache_entry_diesel(
    conn: &mut diesel::SqliteConnection,
    entry_key: &str,
) -> Result<bool> {
    use crate::diesel_schema::cache_index::dsl::*;

    let affected = diesel::delete(cache_index.filter(key.eq(entry_key)))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel delete failed: {}", e))?;

    Ok(affected > 0)
}

/// Check if cache entry exists using Diesel DSL
pub fn has_cache_entry_diesel(
    conn: &mut diesel::SqliteConnection,
    entry_key: &str,
) -> Result<bool> {
    use crate::diesel_schema::cache_index::dsl::*;
    use diesel::dsl::count;

    let cnt: i64 = cache_index
        .filter(key.eq(entry_key))
        .select(count(id))
        .first(conn)
        .map_err(|e| anyhow::anyhow!("Diesel query failed: {}", e))?;

    Ok(cnt > 0)
}

/// Clear all cache entries using Diesel DSL
pub fn clear_all_entries_diesel(conn: &mut diesel::SqliteConnection) -> Result<()> {
    use crate::diesel_schema::cache_index::dsl::*;

    diesel::delete(cache_index)
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel delete failed: {}", e))?;

    Ok(())
}
