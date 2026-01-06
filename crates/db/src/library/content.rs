//! Binary content linked to product metadata (stored in cacache).
//!
//! This tracks images (cover, screenshots, thumbnails) that are stored
//! in cacache, linked to their parent ProductMetadata.

use anyhow::Result;
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

// ============================================================================
// Diesel DSL versions (Promoted to Primary)
// ============================================================================

use diesel::prelude::*;
use diesel::result::OptionalExtension;

/// Diesel-compatible product content row
#[derive(
    Debug, Clone, diesel::Queryable, diesel::Selectable, diesel::Insertable, diesel::AsChangeset,
)]
#[diesel(table_name = crate::diesel_schema::product_content)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct DbProductContent {
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

#[derive(diesel::Insertable, diesel::AsChangeset)]
#[diesel(table_name = crate::diesel_schema::product_content)]
pub struct NewDbProductContent<'a> {
    pub product_id: &'a str,
    pub content_type: &'a str,
    pub content_index: i32,
    pub content_hash: &'a str,
    pub source_url: Option<&'a str>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub size_bytes: Option<i64>,
    pub cached_at: i64,
}

impl DbProductContent {
    pub fn to_product_content(self) -> ProductContent {
        ProductContent {
            id: self.id as i64,
            product_id: self.product_id,
            content_type: self.content_type,
            content_index: self.content_index as i64,
            content_hash: self.content_hash,
            source_url: self.source_url,
            width: self.width.map(|w| w as i64),
            height: self.height.map(|h| h as i64),
            size_bytes: self.size_bytes,
            cached_at: self.cached_at,
        }
    }
}

/// Delete all content for a product
pub fn delete_product_content(conn: &mut diesel::SqliteConnection, prod_id: &str) -> Result<()> {
    use crate::diesel_schema::product_content::dsl::*;

    diesel::delete(product_content.filter(product_id.eq(prod_id)))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel delete failed: {}", e))?;

    Ok(())
}

/// Get all content for a product
pub fn get_all_content(
    conn: &mut diesel::SqliteConnection,
    prod_id: &str,
) -> Result<Vec<ProductContent>> {
    use crate::diesel_schema::product_content::dsl::*;

    let rows = product_content
        .filter(product_id.eq(prod_id))
        .order((content_type.asc(), content_index.asc()))
        .select(DbProductContent::as_select())
        .load::<DbProductContent>(conn)
        .map_err(|e| anyhow::anyhow!("Diesel query failed: {}", e))?;

    Ok(rows.into_iter().map(|r| r.to_product_content()).collect())
}

/// Save product content (upsert)
pub fn save(conn: &mut diesel::SqliteConnection, c: &ProductContent) -> Result<i64> {
    use crate::diesel_schema::product_content::dsl::*;

    let new_content = NewDbProductContent {
        product_id: &c.product_id,
        content_type: &c.content_type,
        content_index: c.content_index as i32,
        content_hash: &c.content_hash,
        source_url: c.source_url.as_deref(),
        width: c.width.map(|w| w as i32),
        height: c.height.map(|h| h as i32),
        size_bytes: c.size_bytes,
        cached_at: c.cached_at,
    };

    diesel::insert_into(product_content)
        .values(&new_content)
        .on_conflict((product_id, content_type, content_index))
        .do_update()
        .set(&new_content)
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel save failed: {}", e))?;

    // Returning last inserted ID in Diesel is backend specific and tricky with upserts in SQLite.
    // For now, we can just return 0 or fetch it if strictly necessary.
    // The legacy code returned last_insert_rowid().
    Ok(0)
}

/// Get cover image for a product
pub fn get_cover(
    conn: &mut diesel::SqliteConnection,
    prod_id: &str,
) -> Result<Option<ProductContent>> {
    use crate::diesel_schema::product_content::dsl::*;

    let result = product_content
        .filter(
            product_id
                .eq(prod_id)
                .and(content_type.eq(ContentType::Cover.as_str())),
        )
        .select(DbProductContent::as_select())
        .first::<DbProductContent>(conn)
        .optional()
        .map_err(|e| anyhow::anyhow!("Diesel get_cover failed: {}", e))?;

    Ok(result.map(|r| r.to_product_content()))
}

/// Get all screenshots for a product
pub fn get_screenshots(
    conn: &mut diesel::SqliteConnection,
    prod_id: &str,
) -> Result<Vec<ProductContent>> {
    use crate::diesel_schema::product_content::dsl::*;

    let rows = product_content
        .filter(
            product_id
                .eq(prod_id)
                .and(content_type.eq(ContentType::Screenshot.as_str())),
        )
        .order(content_index.asc())
        .select(DbProductContent::as_select())
        .load::<DbProductContent>(conn)
        .map_err(|e| anyhow::anyhow!("Diesel get_screenshots failed: {}", e))?;

    Ok(rows.into_iter().map(|r| r.to_product_content()).collect())
}
