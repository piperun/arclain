//! Cache index for tracking cached content locations in SQLite
//!
//! This module provides a SQLite-backed index that tracks where cached
//! content is stored in the cacache content-addressable store.

use anyhow::Result;

/// Type of cached content
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheType {
    /// Image screenshots/samples
    Screenshot,
    /// Thumbnail images
    Thumbnail,
    /// Structured metadata (JSON)
    Metadata,
    /// Raw HTML pages
    Html,
    /// Cover/main images
    Cover,
    /// Other/unknown content
    Other,
}

impl CacheType {
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheType::Screenshot => "screenshot",
            CacheType::Thumbnail => "thumbnail",
            CacheType::Metadata => "metadata",
            CacheType::Html => "html",
            CacheType::Cover => "cover",
            CacheType::Other => "other",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "screenshot" => CacheType::Screenshot,
            "thumbnail" => CacheType::Thumbnail,
            "metadata" => CacheType::Metadata,
            "html" => CacheType::Html,
            "cover" => CacheType::Cover,
            _ => CacheType::Other,
        }
    }

    /// Infer cache type from a cache key
    pub fn from_key(key: &str) -> Self {
        if key.contains(":html:") || key.ends_with(":html") {
            CacheType::Html
        } else if key.contains(":json:") || key.ends_with(":json") {
            CacheType::Metadata
        } else if key.contains(":cover") {
            CacheType::Cover
        } else if key.contains(":screenshot") || key.contains(":sample") {
            CacheType::Screenshot
        } else if key.contains(":thumbnail") || key.contains(":thumb") {
            CacheType::Thumbnail
        } else {
            CacheType::Other
        }
    }

    /// Extract product_id from a cache key if possible
    /// Expected format: "provider:product_id:asset_type" or "provider:type:product_id"
    pub fn extract_product_id(key: &str) -> Option<String> {
        let parts: Vec<&str> = key.split(':').collect();
        if parts.len() >= 2 {
            // Handle "dlsite:RJ123456:cover" format
            let candidate = parts[1];
            // Check if it looks like a product ID (starts with RJ/VJ/BJ or is alphanumeric)
            if candidate.starts_with("RJ")
                || candidate.starts_with("VJ")
                || candidate.starts_with("BJ")
                || (candidate.chars().all(|c| c.is_alphanumeric()) && candidate.len() > 4)
            {
                // Skip if it's a type indicator
                if candidate != "html" && candidate != "json" && candidate != "search" {
                    return Some(format!("{}:{}", parts[0], candidate));
                }
            }
            // Handle "dlsite:html:RJ123456" format
            if parts.len() >= 3 && (parts[1] == "html" || parts[1] == "json") {
                return Some(format!("{}:{}", parts[0], parts[2]));
            }
        }
        None
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

/// Delete cache entries matching a pattern (SQL LIKE)
pub fn delete_by_pattern(conn: &mut diesel::SqliteConnection, pattern: &str) -> Result<usize> {
    use crate::diesel_schema::cache_index::dsl::*;

    // Replace '*' with '%' for SQL LIKE if it's a glob pattern
    // The plugin sends "dlsite:*", we want "dlsite:%" for SQL
    let sql_pattern = pattern.replace('*', "%");

    let affected = diesel::delete(cache_index.filter(key.like(sql_pattern)))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel delete_by_pattern failed: {}", e))?;

    Ok(affected)
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

// ============================================================================
// Garbage Collection & Statistics
// ============================================================================

/// Statistics about the cache
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub total_entries: i64,
    pub total_size_bytes: i64,
    pub entries_by_type: Vec<(String, i64)>,
    pub entries_without_product_id: i64,
    pub orphaned_entries: i64, // Entries with product_id not in product_metadata
    pub search_cache_entries: i64,
    pub oldest_entry_date: Option<String>,
}

/// Get cache statistics
pub fn get_cache_stats(conn: &mut diesel::SqliteConnection) -> Result<CacheStats> {
    use crate::diesel_schema::cache_index::dsl::*;
    use diesel::dsl::count;

    let total_entries: i64 = cache_index
        .select(count(id))
        .first(conn)
        .map_err(|e| anyhow::anyhow!("Failed to count entries: {}", e))?;

    // Use raw SQL for sum to avoid type issues
    let total_size: i64 = diesel::sql_query(
        "SELECT COALESCE(SUM(size_bytes), 0) as cnt FROM cache_index"
    )
    .load::<CountResult>(conn)
    .map(|rows| rows.first().map(|r| r.cnt).unwrap_or(0))
    .unwrap_or(0);

    // Count entries without product_id
    let entries_without_product_id: i64 = cache_index
        .filter(product_id.is_null())
        .select(count(id))
        .first(conn)
        .map_err(|e| anyhow::anyhow!("Failed to count null product_id: {}", e))?;

    // Count search cache entries
    let search_cache_entries: i64 = cache_index
        .filter(key.like("%:search:%"))
        .select(count(id))
        .first(conn)
        .map_err(|e| anyhow::anyhow!("Failed to count search entries: {}", e))?;

    // Count entries by type
    let entries_by_type: Vec<(String, i64)> = diesel::sql_query(
        "SELECT cache_type, COUNT(*) as cnt FROM cache_index GROUP BY cache_type ORDER BY cnt DESC"
    )
    .load::<CacheTypeCount>(conn)
    .map(|rows| rows.into_iter().map(|r| (r.cache_type, r.cnt)).collect())
    .unwrap_or_default();

    // Get oldest entry
    let oldest_entry_date: Option<String> = cache_index
        .select(created_at)
        .order(created_at.asc())
        .first(conn)
        .optional()
        .map_err(|e| anyhow::anyhow!("Failed to get oldest entry: {}", e))?;

    // Count orphaned entries (product_id set but not in product_metadata)
    let orphaned_entries: i64 = diesel::sql_query(
        "SELECT COUNT(*) as cnt FROM cache_index
         WHERE product_id IS NOT NULL
         AND product_id NOT IN (SELECT id FROM product_metadata)"
    )
    .load::<CountResult>(conn)
    .map(|rows| rows.first().map(|r| r.cnt).unwrap_or(0))
    .unwrap_or(0);

    Ok(CacheStats {
        total_entries,
        total_size_bytes: total_size,
        entries_by_type,
        entries_without_product_id,
        orphaned_entries,
        search_cache_entries,
        oldest_entry_date,
    })
}

#[derive(diesel::QueryableByName)]
struct CacheTypeCount {
    #[diesel(sql_type = diesel::sql_types::Text)]
    cache_type: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    cnt: i64,
}

#[derive(diesel::QueryableByName)]
struct CountResult {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    cnt: i64,
}

/// Delete orphaned cache entries (product_id not in product_metadata)
pub fn delete_orphaned_entries(conn: &mut diesel::SqliteConnection) -> Result<usize> {
    let affected = diesel::sql_query(
        "DELETE FROM cache_index
         WHERE product_id IS NOT NULL
         AND product_id NOT IN (SELECT id FROM product_metadata)"
    )
    .execute(conn)
    .map_err(|e| anyhow::anyhow!("Failed to delete orphaned entries: {}", e))?;

    Ok(affected)
}

/// Delete search cache entries older than specified days
pub fn delete_old_search_cache(conn: &mut diesel::SqliteConnection, days: i64) -> Result<usize> {
    let affected = diesel::sql_query(format!(
        "DELETE FROM cache_index
         WHERE key LIKE '%:search:%'
         AND created_at < datetime('now', '-{} days')",
        days
    ))
    .execute(conn)
    .map_err(|e| anyhow::anyhow!("Failed to delete old search cache: {}", e))?;

    Ok(affected)
}

/// Delete ALL search cache entries (search results are ephemeral, shouldn't be cached)
pub fn delete_all_search_cache(conn: &mut diesel::SqliteConnection) -> Result<usize> {
    let affected = diesel::sql_query(
        "DELETE FROM cache_index WHERE key LIKE '%:search:%'",
    )
    .execute(conn)
    .map_err(|e| anyhow::anyhow!("Failed to delete search cache: {}", e))?;

    Ok(affected)
}

/// Get all content hashes currently in the index
pub fn get_all_content_hashes(conn: &mut diesel::SqliteConnection) -> Result<Vec<String>> {
    use crate::diesel_schema::cache_index::dsl::*;

    let hashes = cache_index
        .select(content_hash)
        .distinct()
        .load::<String>(conn)
        .map_err(|e| anyhow::anyhow!("Failed to get content hashes: {}", e))?;

    Ok(hashes)
}

/// Fix cache entries: update cache_type and product_id based on key patterns
pub fn migrate_fix_entries(conn: &mut diesel::SqliteConnection) -> Result<(usize, usize)> {
    // Fix cache_type based on key patterns
    let type_fixed = diesel::sql_query(
        "UPDATE cache_index SET cache_type =
         CASE
             WHEN key LIKE '%:html:%' OR key LIKE '%:html' THEN 'html'
             WHEN key LIKE '%:json:%' OR key LIKE '%:json' THEN 'metadata'
             WHEN key LIKE '%:cover' THEN 'cover'
             WHEN key LIKE '%:screenshot%' OR key LIKE '%:sample%' THEN 'screenshot'
             WHEN key LIKE '%:thumbnail%' OR key LIKE '%:thumb%' THEN 'thumbnail'
             ELSE cache_type
         END
         WHERE cache_type = 'screenshot'
         AND (key LIKE '%:html%' OR key LIKE '%:json%' OR key LIKE '%:cover')"
    )
    .execute(conn)
    .unwrap_or(0);

    // Fix product_id for dlsite entries
    // Pattern: dlsite:RJ123456:asset or dlsite:type:RJ123456
    let product_fixed = diesel::sql_query(
        "UPDATE cache_index SET product_id =
         CASE
             -- dlsite:RJ123456:asset pattern
             WHEN key LIKE 'dlsite:RJ%:%' OR key LIKE 'dlsite:VJ%:%' OR key LIKE 'dlsite:BJ%:%'
             THEN 'dlsite:' || substr(key, 8, instr(substr(key, 8), ':') - 1)
             -- dlsite:html:RJ123456 or dlsite:json:RJ123456 pattern
             WHEN key LIKE 'dlsite:html:%' THEN 'dlsite:' || substr(key, 13)
             WHEN key LIKE 'dlsite:json:%' THEN 'dlsite:' || substr(key, 13)
             ELSE product_id
         END
         WHERE product_id IS NULL
         AND key LIKE 'dlsite:%'
         AND key NOT LIKE 'dlsite:search:%'"
    )
    .execute(conn)
    .unwrap_or(0);

    Ok((type_fixed, product_fixed))
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // CacheType::as_str / from_str round-trip
    // =========================================================================

    #[test]
    fn test_cache_type_roundtrip() {
        let types = [
            CacheType::Screenshot,
            CacheType::Thumbnail,
            CacheType::Metadata,
            CacheType::Html,
            CacheType::Cover,
            CacheType::Other,
        ];
        for ct in &types {
            assert_eq!(CacheType::from_str(ct.as_str()).as_str(), ct.as_str());
        }
    }

    #[test]
    fn test_cache_type_from_str_unknown_falls_back_to_other() {
        assert_eq!(CacheType::from_str("unknown").as_str(), "other");
        assert_eq!(CacheType::from_str("").as_str(), "other");
    }

    // =========================================================================
    // CacheType::from_key
    // =========================================================================

    #[test]
    fn test_from_key_html() {
        assert_eq!(CacheType::from_key("dlsite:html:RJ123456").as_str(), "html");
        assert_eq!(CacheType::from_key("dlsite:RJ123456:html").as_str(), "html");
    }

    #[test]
    fn test_from_key_metadata() {
        assert_eq!(
            CacheType::from_key("dlsite:json:RJ123456").as_str(),
            "metadata"
        );
        assert_eq!(
            CacheType::from_key("dlsite:RJ123456:json").as_str(),
            "metadata"
        );
    }

    #[test]
    fn test_from_key_cover() {
        assert_eq!(
            CacheType::from_key("dlsite:RJ123456:cover").as_str(),
            "cover"
        );
    }

    #[test]
    fn test_from_key_screenshot() {
        assert_eq!(
            CacheType::from_key("dlsite:RJ123456:screenshot_0").as_str(),
            "screenshot"
        );
        assert_eq!(
            CacheType::from_key("dlsite:RJ123456:sample_1").as_str(),
            "screenshot"
        );
    }

    #[test]
    fn test_from_key_thumbnail() {
        assert_eq!(
            CacheType::from_key("dlsite:RJ123456:thumbnail").as_str(),
            "thumbnail"
        );
        assert_eq!(
            CacheType::from_key("dlsite:RJ123456:thumb").as_str(),
            "thumbnail"
        );
    }

    #[test]
    fn test_from_key_other() {
        assert_eq!(
            CacheType::from_key("dlsite:RJ123456:something").as_str(),
            "other"
        );
        assert_eq!(CacheType::from_key("unknown").as_str(), "other");
    }

    // =========================================================================
    // CacheType::extract_product_id
    // =========================================================================

    #[test]
    fn test_extract_product_id_standard_format() {
        // "dlsite:RJ123456:cover" -> "dlsite:RJ123456"
        assert_eq!(
            CacheType::extract_product_id("dlsite:RJ123456:cover"),
            Some("dlsite:RJ123456".to_string())
        );
        assert_eq!(
            CacheType::extract_product_id("dlsite:VJ001234:screenshot_0"),
            Some("dlsite:VJ001234".to_string())
        );
        assert_eq!(
            CacheType::extract_product_id("dlsite:BJ999999:thumb"),
            Some("dlsite:BJ999999".to_string())
        );
    }

    #[test]
    fn test_extract_product_id_html_json_format() {
        // "dlsite:html:RJ123456" -> "dlsite:RJ123456"
        assert_eq!(
            CacheType::extract_product_id("dlsite:html:RJ123456"),
            Some("dlsite:RJ123456".to_string())
        );
        assert_eq!(
            CacheType::extract_product_id("dlsite:json:RJ123456"),
            Some("dlsite:RJ123456".to_string())
        );
    }

    #[test]
    fn test_extract_product_id_no_match() {
        assert_eq!(CacheType::extract_product_id(""), None);
        assert_eq!(CacheType::extract_product_id("single"), None);
        // "dlsite:search:query" — search is skipped
        assert_eq!(CacheType::extract_product_id("dlsite:search:test"), None);
    }

    #[test]
    fn test_extract_product_id_long_alphanumeric() {
        // Non-RJ/VJ/BJ but long enough to be a product ID
        assert_eq!(
            CacheType::extract_product_id("steam:12345678:cover"),
            Some("steam:12345678".to_string())
        );
    }

    // =========================================================================
    // Diesel CRUD operations
    // =========================================================================

    mod diesel_crud {
        use super::super::*;

        fn setup_diesel() -> diesel::SqliteConnection {
            let mut conn = diesel::SqliteConnection::establish(":memory:")
                .expect("in-memory SQLite");
            diesel::sql_query(
                "CREATE TABLE cache_index (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    key TEXT UNIQUE NOT NULL,
                    product_id TEXT,
                    content_hash TEXT NOT NULL,
                    source_url TEXT,
                    cache_type TEXT NOT NULL DEFAULT 'other',
                    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    last_accessed TEXT,
                    size_bytes INTEGER
                )",
            )
            .execute(&mut conn)
            .unwrap();
            // Minimal product_metadata stub for stats queries
            diesel::sql_query(
                "CREATE TABLE product_metadata (id TEXT PRIMARY KEY)",
            )
            .execute(&mut conn)
            .unwrap();
            conn
        }

        #[test]
        fn test_upsert_and_get() {
            let mut conn = setup_diesel();
            let id = upsert_cache_entry(
                &mut conn,
                "dlsite:RJ100:cover",
                Some("dlsite:RJ100"),
                "sha256-abc",
                Some("https://example.com/img.jpg"),
                CacheType::Cover,
                Some(2048),
            )
            .unwrap();
            assert!(id > 0);

            let entry = get_cache_entry(&mut conn, "dlsite:RJ100:cover")
                .unwrap()
                .unwrap();
            assert_eq!(entry.key, "dlsite:RJ100:cover");
            assert_eq!(entry.product_id, Some("dlsite:RJ100".to_string()));
            assert_eq!(entry.content_hash, "sha256-abc");
            assert_eq!(entry.cache_type, CacheType::Cover);
            assert_eq!(entry.size_bytes, Some(2048));
        }

        #[test]
        fn test_upsert_updates_existing() {
            let mut conn = setup_diesel();
            upsert_cache_entry(
                &mut conn,
                "key1",
                None,
                "hash-v1",
                None,
                CacheType::Other,
                Some(100),
            )
            .unwrap();

            upsert_cache_entry(
                &mut conn,
                "key1",
                None,
                "hash-v2",
                None,
                CacheType::Other,
                Some(200),
            )
            .unwrap();

            let entry = get_cache_entry(&mut conn, "key1").unwrap().unwrap();
            assert_eq!(entry.content_hash, "hash-v2");
            assert_eq!(entry.size_bytes, Some(200));
        }

        #[test]
        fn test_get_nonexistent() {
            let mut conn = setup_diesel();
            assert!(get_cache_entry(&mut conn, "nope").unwrap().is_none());
        }

        #[test]
        fn test_has_cache_entry() {
            let mut conn = setup_diesel();
            assert!(!has_cache_entry(&mut conn, "k").unwrap());

            upsert_cache_entry(&mut conn, "k", None, "h", None, CacheType::Other, None)
                .unwrap();
            assert!(has_cache_entry(&mut conn, "k").unwrap());
        }

        #[test]
        fn test_delete_cache_entry() {
            let mut conn = setup_diesel();
            upsert_cache_entry(&mut conn, "del", None, "h", None, CacheType::Other, None)
                .unwrap();
            assert!(delete_cache_entry(&mut conn, "del").unwrap());
            assert!(!has_cache_entry(&mut conn, "del").unwrap());

            // Deleting non-existent returns false
            assert!(!delete_cache_entry(&mut conn, "del").unwrap());
        }

        #[test]
        fn test_get_entries_by_product() {
            let mut conn = setup_diesel();
            upsert_cache_entry(
                &mut conn,
                "dlsite:RJ200:cover",
                Some("dlsite:RJ200"),
                "h1",
                None,
                CacheType::Cover,
                None,
            )
            .unwrap();
            upsert_cache_entry(
                &mut conn,
                "dlsite:RJ200:screenshot_0",
                Some("dlsite:RJ200"),
                "h2",
                None,
                CacheType::Screenshot,
                None,
            )
            .unwrap();
            upsert_cache_entry(
                &mut conn,
                "dlsite:RJ999:cover",
                Some("dlsite:RJ999"),
                "h3",
                None,
                CacheType::Cover,
                None,
            )
            .unwrap();

            let entries = get_entries_by_product(&mut conn, "dlsite:RJ200").unwrap();
            assert_eq!(entries.len(), 2);

            let entries = get_entries_by_product(&mut conn, "dlsite:RJ999").unwrap();
            assert_eq!(entries.len(), 1);
        }

        #[test]
        fn test_delete_by_pattern() {
            let mut conn = setup_diesel();
            upsert_cache_entry(
                &mut conn,
                "dlsite:RJ300:cover",
                None,
                "h",
                None,
                CacheType::Cover,
                None,
            )
            .unwrap();
            upsert_cache_entry(
                &mut conn,
                "dlsite:RJ300:html",
                None,
                "h",
                None,
                CacheType::Html,
                None,
            )
            .unwrap();
            upsert_cache_entry(
                &mut conn,
                "steam:123:cover",
                None,
                "h",
                None,
                CacheType::Cover,
                None,
            )
            .unwrap();

            let deleted = delete_by_pattern(&mut conn, "dlsite:*").unwrap();
            assert_eq!(deleted, 2);
            assert!(has_cache_entry(&mut conn, "steam:123:cover").unwrap());
        }

        #[test]
        fn test_clear_all_entries() {
            let mut conn = setup_diesel();
            upsert_cache_entry(&mut conn, "a", None, "h", None, CacheType::Other, None)
                .unwrap();
            upsert_cache_entry(&mut conn, "b", None, "h", None, CacheType::Other, None)
                .unwrap();

            clear_all_entries(&mut conn).unwrap();
            assert!(!has_cache_entry(&mut conn, "a").unwrap());
            assert!(!has_cache_entry(&mut conn, "b").unwrap());
        }

        #[test]
        fn test_touch_cache_entry() {
            let mut conn = setup_diesel();
            upsert_cache_entry(&mut conn, "t", None, "h", None, CacheType::Other, None)
                .unwrap();

            let before = get_cache_entry(&mut conn, "t").unwrap().unwrap();
            assert!(before.last_accessed.is_none());

            touch_cache_entry(&mut conn, "t").unwrap();
            let after = get_cache_entry(&mut conn, "t").unwrap().unwrap();
            assert!(after.last_accessed.is_some());
        }

        #[test]
        fn test_get_all_content_hashes() {
            let mut conn = setup_diesel();
            upsert_cache_entry(&mut conn, "a", None, "hash1", None, CacheType::Other, None)
                .unwrap();
            upsert_cache_entry(&mut conn, "b", None, "hash2", None, CacheType::Other, None)
                .unwrap();
            upsert_cache_entry(&mut conn, "c", None, "hash1", None, CacheType::Other, None)
                .unwrap();

            let hashes = get_all_content_hashes(&mut conn).unwrap();
            assert_eq!(hashes.len(), 2); // distinct
            assert!(hashes.contains(&"hash1".to_string()));
            assert!(hashes.contains(&"hash2".to_string()));
        }

        #[test]
        fn test_get_cache_stats_empty() {
            let mut conn = setup_diesel();
            let stats = get_cache_stats(&mut conn).unwrap();
            assert_eq!(stats.total_entries, 0);
            assert_eq!(stats.total_size_bytes, 0);
            assert!(stats.oldest_entry_date.is_none());
        }

        #[test]
        fn test_get_cache_stats_with_data() {
            let mut conn = setup_diesel();
            upsert_cache_entry(
                &mut conn,
                "dlsite:RJ100:cover",
                Some("dlsite:RJ100"),
                "h1",
                None,
                CacheType::Cover,
                Some(1000),
            )
            .unwrap();
            upsert_cache_entry(
                &mut conn,
                "dlsite:search:query1",
                None,
                "h2",
                None,
                CacheType::Other,
                Some(500),
            )
            .unwrap();

            let stats = get_cache_stats(&mut conn).unwrap();
            assert_eq!(stats.total_entries, 2);
            assert_eq!(stats.total_size_bytes, 1500);
            assert_eq!(stats.search_cache_entries, 1);
            assert!(stats.oldest_entry_date.is_some());
        }
    }
}
