//! Binary content linked to product metadata (stored in cacache).
//!
//! This tracks images (cover, screenshots, thumbnails) that are stored
//! in cacache, linked to their parent ProductMetadata.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::time::{SystemTime, UNIX_EPOCH};

/// Type of cached content
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentType {
    #[default]
    Cover,
    Screenshot,
    Thumbnail,
}

impl ContentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContentType::Cover => "cover",
            ContentType::Screenshot => "screenshot",
            ContentType::Thumbnail => "thumbnail",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "cover" => ContentType::Cover,
            "screenshot" => ContentType::Screenshot,
            "thumbnail" => ContentType::Thumbnail,
            _ => ContentType::Cover,
        }
    }
}

/// Binary content linked to a product (stored in cacache).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProductContent {
    pub id: i64,
    /// Foreign key to product_metadata.id
    pub product_id: String,
    /// Content type: "cover", "screenshot", "thumbnail"
    pub content_type: String,
    /// Index for ordered content (0 for cover, 0-N for screenshots)
    pub content_index: i64,
    /// cacache integrity hash
    pub content_hash: String,
    pub source_url: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub size_bytes: Option<i64>,
    pub cached_at: i64,
}

impl ProductContent {
    /// Create a new ProductContent entry
    pub fn new(product_id: &str, content_type: ContentType, content_hash: &str) -> Self {
        Self {
            id: 0, // Auto-assigned by DB
            product_id: product_id.to_string(),
            content_type: content_type.as_str().to_string(),
            content_index: 0,
            content_hash: content_hash.to_string(),
            cached_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            ..Default::default()
        }
    }

    /// Create a screenshot entry with index
    pub fn screenshot(product_id: &str, index: i64, content_hash: &str) -> Self {
        Self {
            id: 0,
            product_id: product_id.to_string(),
            content_type: ContentType::Screenshot.as_str().to_string(),
            content_index: index,
            content_hash: content_hash.to_string(),
            cached_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            ..Default::default()
        }
    }

    /// Get the content type as an enum
    pub fn get_content_type(&self) -> ContentType {
        ContentType::from_str(&self.content_type)
    }

    /// Parse from a database row
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            product_id: row.get(1)?,
            content_type: row.get(2)?,
            content_index: row.get(3)?,
            content_hash: row.get(4)?,
            source_url: row.get(5).ok(),
            width: row.get(6).ok(),
            height: row.get(7).ok(),
            size_bytes: row.get(8).ok(),
            cached_at: row.get(9)?,
        })
    }
}

/// SQL to create the product_content table
pub const CREATE_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS product_content (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id TEXT NOT NULL,
    content_type TEXT NOT NULL,
    content_index INTEGER NOT NULL DEFAULT 0,
    content_hash TEXT NOT NULL,
    source_url TEXT,
    width INTEGER,
    height INTEGER,
    size_bytes INTEGER,
    cached_at INTEGER NOT NULL,
    UNIQUE (product_id, content_type, content_index)
);
CREATE INDEX IF NOT EXISTS idx_content_product ON product_content(product_id);
CREATE INDEX IF NOT EXISTS idx_content_type ON product_content(content_type);
"#;

const SELECT_COLS: &str =
    "id, product_id, content_type, content_index, content_hash, source_url, width, height, size_bytes, cached_at";

/// Initialize the product_content table
pub fn init_product_content_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(CREATE_TABLE_SQL)?;
    Ok(())
}

/// Save product content (upsert)
pub fn save(conn: &Connection, c: &ProductContent) -> Result<i64> {
    conn.execute(
        "INSERT OR REPLACE INTO product_content (
            product_id, content_type, content_index, content_hash, source_url, width, height, size_bytes, cached_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            &c.product_id,
            &c.content_type,
            &c.content_index,
            &c.content_hash,
            &c.source_url,
            &c.width,
            &c.height,
            &c.size_bytes,
            &c.cached_at
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Get cover image for a product
pub fn get_cover(conn: &Connection, product_id: &str) -> Result<Option<ProductContent>> {
    let sql = format!(
        "SELECT {} FROM product_content WHERE product_id = ?1 AND content_type = ?2 LIMIT 1",
        SELECT_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let entry = stmt
        .query_row(
            [product_id, ContentType::Cover.as_str()],
            ProductContent::from_row,
        )
        .optional()?;
    Ok(entry)
}

/// Get all screenshots for a product, ordered by index
pub fn get_screenshots(conn: &Connection, product_id: &str) -> Result<Vec<ProductContent>> {
    let sql = format!(
        "SELECT {} FROM product_content WHERE product_id = ?1 AND content_type = ?2 ORDER BY content_index ASC",
        SELECT_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        [product_id, ContentType::Screenshot.as_str()],
        ProductContent::from_row,
    )?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Get all content for a product
pub fn get_all_content(conn: &Connection, product_id: &str) -> Result<Vec<ProductContent>> {
    let sql = format!(
        "SELECT {} FROM product_content WHERE product_id = ?1 ORDER BY content_type, content_index ASC",
        SELECT_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([product_id], ProductContent::from_row)?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Delete all content for a product
pub fn delete_product_content(conn: &Connection, product_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM product_content WHERE product_id = ?1",
        [product_id],
    )?;
    Ok(())
}

// ============================================================================
// Diesel DSL versions
// ============================================================================

use diesel::prelude::*;

/// Diesel-compatible product content row
#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::diesel_schema::product_content)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct DbProductContentRow {
    pub id: i32,
    pub product_id: String,
    pub content_type: String,
    pub content_index: i32,
    pub content_hash: String,
    pub source_url: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub size_bytes: Option<i64>,
    pub cached_at: i64,
}

impl DbProductContentRow {
    pub fn to_product_content(&self) -> ProductContent {
        ProductContent {
            id: self.id as i64,
            product_id: self.product_id.clone(),
            content_type: self.content_type.clone(),
            content_index: self.content_index as i64,
            content_hash: self.content_hash.clone(),
            source_url: self.source_url.clone(),
            width: self.width.map(|w| w as i64),
            height: self.height.map(|h| h as i64),
            size_bytes: self.size_bytes,
            cached_at: self.cached_at,
        }
    }
}

/// Delete all content for a product using Diesel DSL
pub fn delete_product_content_diesel(
    conn: &mut diesel::SqliteConnection,
    prod_id: &str,
) -> Result<()> {
    use crate::diesel_schema::product_content::dsl::*;

    diesel::delete(product_content.filter(product_id.eq(prod_id)))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel delete failed: {}", e))?;

    Ok(())
}

/// Get all content for a product using Diesel DSL
pub fn get_all_content_diesel(
    conn: &mut diesel::SqliteConnection,
    prod_id: &str,
) -> Result<Vec<ProductContent>> {
    use crate::diesel_schema::product_content::dsl::*;

    let rows = product_content
        .filter(product_id.eq(prod_id))
        .order((content_type.asc(), content_index.asc()))
        .load::<DbProductContentRow>(conn)
        .map_err(|e| anyhow::anyhow!("Diesel query failed: {}", e))?;

    Ok(rows.iter().map(|r| r.to_product_content()).collect())
}
