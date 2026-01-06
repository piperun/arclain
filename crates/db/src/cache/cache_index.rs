//! Cache index for tracking cached content locations in SQLite
//!
//! This module provides a SQLite-backed index that tracks where cached
//! content is stored in the cacache content-addressable store.

use anyhow::Result;

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

// ============================================================================
// Diesel DSL versions (Primary)
// ============================================================================

use diesel::prelude::*;
use diesel::result::OptionalExtension;

/// Diesel-compatible cache index row
#[derive(
    Debug, Clone, diesel::Queryable, diesel::Selectable, diesel::Insertable, diesel::AsChangeset,
)]
#[diesel(table_name = crate::diesel_schema::cache_index)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct DbCacheEntry {
    pub id: i32,
    pub key: String,
    pub product_id: Option<String>,
    pub content_hash: String,
    pub source_url: Option<String>,
    pub cache_type: String,
    pub created_at: String, // DB sets default, but we might read it
    pub last_accessed: Option<String>,
    pub size_bytes: Option<i64>,
}

#[derive(diesel::Insertable, diesel::AsChangeset)]
#[diesel(table_name = crate::diesel_schema::cache_index)]
pub struct NewDbCacheEntry<'a> {
    pub key: &'a str,
    pub product_id: Option<&'a str>,
    pub content_hash: &'a str,
    pub source_url: Option<&'a str>,
    pub cache_type: &'a str,
    pub size_bytes: Option<i64>,
    // created_at usually default, unless we set it explicitly?
    // The previous code let DB set CURRENT_TIMESTAMP on insert.
    // So we omit it from insert struct.
}

#[derive(diesel::AsChangeset)]
#[diesel(table_name = crate::diesel_schema::cache_index)]
pub struct UpdateDbCacheEntry<'a> {
    pub content_hash: &'a str,
    pub source_url: Option<&'a str>,
    // We want to set last_accessed = CURRENT_TIMESTAMP.
    // Diesel doesn't easily support "CURRENT_TIMESTAMP" literal in AsChangeset without expression.
    // We can use custom update or just set it to now in Rust.
    // Previous code used `last_accessed = CURRENT_TIMESTAMP`.
    pub size_bytes: Option<i64>,
}

impl DbCacheEntry {
    pub fn to_cache_entry(self) -> CacheEntry {
        CacheEntry {
            id: self.id as i64,
            key: self.key,
            product_id: self.product_id,
            content_hash: self.content_hash,
            source_url: self.source_url,
            cache_type: CacheType::from_str(&self.cache_type),
            created_at: self.created_at,
            last_accessed: self.last_accessed,
            size_bytes: self.size_bytes,
        }
    }
}

/// Insert or update a cache entry
pub fn upsert_cache_entry(
    conn: &mut diesel::SqliteConnection,
    key: &str,
    product_id: Option<&str>,
    content_hash: &str,
    source_url: Option<&str>,
    cache_type: CacheType,
    size_bytes: Option<i64>,
) -> Result<i64> {
    use crate::diesel_schema::cache_index::dsl;

    // We can't use simple standard struct with CURRENT_TIMESTAMP easily in one go if we also want to return ID.
    // But we can just use normal insert.

    // Manually handle upsert because we need to set last_accessed = CURRENT_TIMESTAMP on conflict.
    // Diesel's `do_update().set(..)` works.
    // We can use `diesel::dsl::now` but sqlite stores strings.
    // Actually rust `chrono::Utc::now()` is safer for consistency if we use ISO string everywhere.
    // But previous code used `CURRENT_TIMESTAMP` (SQLite default).
    // Let's stick to `CURRENT_TIMESTAMP` via `diesel::dsl::sql`.

    diesel::insert_into(dsl::cache_index)
        .values((
            dsl::key.eq(key),
            dsl::product_id.eq(product_id),
            dsl::content_hash.eq(content_hash),
            dsl::source_url.eq(source_url),
            dsl::cache_type.eq(cache_type.as_str()),
            dsl::size_bytes.eq(size_bytes),
            // created_at defaults
        ))
        .on_conflict(dsl::key)
        .do_update()
        .set((
            dsl::content_hash.eq(content_hash),
            dsl::source_url.eq(source_url),
            dsl::last_accessed.eq(diesel::dsl::sql("CURRENT_TIMESTAMP")),
            dsl::size_bytes.eq(size_bytes),
        ))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel upsert failed: {}", e))?;

    // Fetch ID
    let id: i32 = dsl::cache_index
        .filter(dsl::key.eq(key))
        .select(dsl::id)
        .first(conn)?;

    Ok(id as i64)
}

/// Get a cache entry by key
pub fn get_cache_entry(
    conn: &mut diesel::SqliteConnection,
    key_param: &str,
) -> Result<Option<CacheEntry>> {
    use crate::diesel_schema::cache_index::dsl::*;

    let entry = cache_index
        .filter(key.eq(key_param))
        .select(DbCacheEntry::as_select())
        .first::<DbCacheEntry>(conn)
        .optional()
        .map_err(|e| anyhow::anyhow!("Diesel get failed: {}", e))?;

    Ok(entry.map(|e| e.to_cache_entry()))
}

/// Get all cache entries for a product
pub fn get_entries_by_product(
    conn: &mut diesel::SqliteConnection,
    prod_id: &str,
) -> Result<Vec<CacheEntry>> {
    use crate::diesel_schema::cache_index::dsl::*;

    let entries = cache_index
        .filter(product_id.eq(prod_id))
        .order(created_at.desc())
        .select(DbCacheEntry::as_select())
        .load::<DbCacheEntry>(conn)
        .map_err(|e| anyhow::anyhow!("Diesel get_entries failed: {}", e))?;

    Ok(entries.into_iter().map(|e| e.to_cache_entry()).collect())
}

/// Check if a cache entry exists
pub fn has_cache_entry(conn: &mut diesel::SqliteConnection, key_param: &str) -> Result<bool> {
    use crate::diesel_schema::cache_index::dsl::*;
    use diesel::dsl::count;

    let cnt: i64 = cache_index
        .filter(key.eq(key_param))
        .select(count(id))
        .first(conn)
        .map_err(|e| anyhow::anyhow!("Diesel count failed: {}", e))?;

    Ok(cnt > 0)
}

/// Delete a cache entry by key
pub fn delete_cache_entry(conn: &mut diesel::SqliteConnection, key_param: &str) -> Result<bool> {
    use crate::diesel_schema::cache_index::dsl::*;

    let affected = diesel::delete(cache_index.filter(key.eq(key_param)))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel delete failed: {}", e))?;

    Ok(affected > 0)
}

/// Update last_accessed timestamp
pub fn touch_cache_entry(conn: &mut diesel::SqliteConnection, key_param: &str) -> Result<()> {
    use crate::diesel_schema::cache_index::dsl::*;

    diesel::update(cache_index.filter(key.eq(key_param)))
        .set(last_accessed.eq(diesel::dsl::sql("CURRENT_TIMESTAMP")))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel touch failed: {}", e))?;

    Ok(())
}

/// Remove all cache entries
pub fn clear_all_entries(conn: &mut diesel::SqliteConnection) -> Result<()> {
    use crate::diesel_schema::cache_index::dsl::*;

    diesel::delete(cache_index)
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel clear failed: {}", e))?;

    Ok(())
}
