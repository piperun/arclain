//! Unified product metadata stored in the database.
//!
//! This stores metadata from any source (DLSite, itch.io, Steam, etc.) in a single table.

use anyhow::Result;
use chrono::Utc;
/// Get current time as ISO 8601 string
fn now_iso8601() -> String {
    Utc::now().to_rfc3339()
}

/// Metadata source platform
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetadataSource {
    #[default]
    DLSite,
    Itch,
    Steam,
    Custom,
}

impl MetadataSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            MetadataSource::DLSite => "dlsite",
            MetadataSource::Itch => "itch",
            MetadataSource::Steam => "steam",
            MetadataSource::Custom => "custom",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "dlsite" => MetadataSource::DLSite,
            "itch" => MetadataSource::Itch,
            "steam" => MetadataSource::Steam,
            _ => MetadataSource::Custom,
        }
    }
}

/// Unified product metadata stored in the database.
///
/// All metadata from any source (DLSite, itch.io, Steam, etc.) is stored here.
#[derive(
    Debug,
    Clone,
    serde::Serialize,
    serde::Deserialize,
    diesel::Queryable,
    diesel::Selectable,
    diesel::Insertable,
    diesel::AsChangeset,
)]
#[diesel(table_name = crate::diesel_schema::product_metadata)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct ProductMetadata {
    /// Primary key: "dlsite:RJ123456", "itch:12345", etc.
    pub id: String,
    /// Source platform: "dlsite", "itch", "steam"
    pub source: String,
    /// External ID on the platform: "RJ123456", "12345"
    pub external_id: String,

    // --- Core fields (common across sources) ---
    pub title: Option<String>,
    /// Circle/author/developer
    pub creator: Option<String>,
    pub description: Option<String>,
    pub release_date: Option<String>,
    /// Price in smallest unit (yen, cents)
    pub price: Option<i64>,
    /// Currency code: "JPY", "USD"
    pub currency: Option<String>,

    // --- Ratings and stats ---
    /// Average rating (e.g. 4.54)
    pub rating: Option<f64>,
    pub rating_count: Option<i64>,
    pub purchase_count: Option<i64>,
    pub favorite_count: Option<i64>,
    pub review_count: Option<i64>,

    // --- Content info ---
    pub file_size: Option<String>,
    pub file_format: Option<String>,
    pub age_rating: Option<String>,

    // --- Multi-value fields (JSON arrays) ---
    pub genres_json: Option<String>,
    pub tags_json: Option<String>,
    pub languages_json: Option<String>,
    pub product_formats_json: Option<String>,

    // --- DLSite-specific fields ---
    pub series_name: Option<String>,
    pub illustrator: Option<String>,
    pub voice_actors_json: Option<String>,
    pub miscellaneous: Option<String>,
    pub update_info: Option<String>,
    pub rankings_json: Option<String>,

    // --- Source-specific extras ---
    pub extras_json: Option<String>,

    // --- Raw data storage ---
    pub raw_api_response: Option<String>,
    pub raw_html: Option<String>,

    // --- Fetch status ---
    /// Whether the product page was geo-blocked when fetched
    pub geo_blocked: Option<bool>,

    // --- Timestamps (ISO 8601 format) ---
    pub cached_at: String,
    pub updated_at: Option<String>,
    pub last_accessed: Option<String>,
}

impl Default for ProductMetadata {
    fn default() -> Self {
        Self {
            id: String::new(),
            source: String::new(),
            external_id: String::new(),
            title: None,
            creator: None,
            description: None,
            release_date: None,
            price: None,
            currency: None,
            rating: None,
            rating_count: None,
            purchase_count: None,
            favorite_count: None,
            review_count: None,
            file_size: None,
            file_format: None,
            age_rating: None,
            genres_json: None,
            tags_json: None,
            languages_json: None,
            product_formats_json: None,
            series_name: None,
            illustrator: None,
            voice_actors_json: None,
            miscellaneous: None,
            update_info: None,
            rankings_json: None,
            extras_json: None,
            raw_api_response: None,
            raw_html: None,
            geo_blocked: None,
            cached_at: now_iso8601(),
            updated_at: None,
            last_accessed: None,
        }
    }
}

impl ProductMetadata {
    /// Create a new ProductMetadata with the given source and external ID
    pub fn new(source: MetadataSource, external_id: &str) -> Self {
        let id = format!("{}:{}", source.as_str(), external_id);
        Self {
            id,
            source: source.as_str().to_string(),
            external_id: external_id.to_string(),
            cached_at: now_iso8601(),
            ..Default::default()
        }
    }

    /// Get the metadata source as an enum
    pub fn get_source(&self) -> MetadataSource {
        MetadataSource::from_str(&self.source)
    }

    // --- JSON array helpers ---

    pub fn get_genres(&self) -> Vec<String> {
        self.genres_json
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    pub fn set_genres(&mut self, genres: &[String]) {
        self.genres_json = serde_json::to_string(genres).ok();
    }

    pub fn get_tags(&self) -> Vec<String> {
        self.tags_json
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    pub fn set_tags(&mut self, tags: &[String]) {
        self.tags_json = serde_json::to_string(tags).ok();
    }

    pub fn get_languages(&self) -> Vec<String> {
        self.languages_json
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    pub fn set_languages(&mut self, languages: &[String]) {
        self.languages_json = serde_json::to_string(languages).ok();
    }

    pub fn get_product_formats(&self) -> Vec<String> {
        self.product_formats_json
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    pub fn set_product_formats(&mut self, formats: &[String]) {
        self.product_formats_json = serde_json::to_string(formats).ok();
    }

    pub fn get_voice_actors(&self) -> Vec<String> {
        self.voice_actors_json
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    pub fn set_voice_actors(&mut self, actors: &[String]) {
        self.voice_actors_json = serde_json::to_string(actors).ok();
    }

    /// Touch last_accessed timestamp
    pub fn touch(&mut self) {
        self.last_accessed = Some(now_iso8601());
    }
}

/// SQL to create the product_metadata table
pub const CREATE_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS product_metadata (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    external_id TEXT NOT NULL,
    title TEXT,
    creator TEXT,
    description TEXT,
    release_date TEXT,
    price INTEGER,
    currency TEXT,
    rating REAL,
    rating_count INTEGER,
    purchase_count INTEGER,
    favorite_count INTEGER,
    review_count INTEGER,
    file_size TEXT,
    file_format TEXT,
    age_rating TEXT,
    genres_json TEXT,
    tags_json TEXT,
    languages_json TEXT,
    product_formats_json TEXT,
    series_name TEXT,
    illustrator TEXT,
    voice_actors_json TEXT,
    miscellaneous TEXT,
    update_info TEXT,
    rankings_json TEXT,
    extras_json TEXT,
    raw_api_response TEXT,
    raw_html TEXT,
    geo_blocked INTEGER,
    cached_at TEXT NOT NULL,
    updated_at TEXT,
    last_accessed TEXT
);
CREATE INDEX IF NOT EXISTS idx_product_source ON product_metadata(source);
CREATE INDEX IF NOT EXISTS idx_product_external ON product_metadata(external_id);
"#;

// ============================================================================
// Diesel DSL versions (Promoted to Primary)
// ============================================================================

use diesel::prelude::*;
use diesel::result::OptionalExtension;

/// Delete product metadata by ID
pub fn delete(conn: &mut diesel::SqliteConnection, product_id: &str) -> Result<()> {
    use crate::diesel_schema::product_metadata::dsl::*;

    diesel::delete(product_metadata.filter(id.eq(product_id)))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel delete failed: {}", e))?;

    Ok(())
}

/// Check if product exists
#[allow(dead_code)]
pub fn exists(conn: &mut diesel::SqliteConnection, product_id: &str) -> Result<bool> {
    use crate::diesel_schema::product_metadata::dsl::*;
    use diesel::dsl::count;

    let cnt: i64 = product_metadata
        .filter(id.eq(product_id))
        .select(count(id))
        .first(conn)
        .map_err(|e| anyhow::anyhow!("Diesel count failed: {}", e))?;

    Ok(cnt > 0)
}

/// List product IDs by source
pub fn list_ids_by_source(
    conn: &mut diesel::SqliteConnection,
    src: MetadataSource,
) -> Result<Vec<String>> {
    use crate::diesel_schema::product_metadata::dsl::*;

    let ids = product_metadata
        .filter(source.eq(src.as_str()))
        .select(id)
        .load::<String>(conn)
        .map_err(|e| anyhow::anyhow!("Diesel query failed: {}", e))?;

    Ok(ids)
}

/// Load product metadata by ID
pub fn load(
    conn: &mut diesel::SqliteConnection,
    product_id: &str,
) -> Result<Option<ProductMetadata>> {
    use crate::diesel_schema::product_metadata::dsl::*;

    let result = product_metadata
        .find(product_id)
        .select(ProductMetadata::as_select())
        .first(conn)
        .optional()
        .map_err(|e| anyhow::anyhow!("Diesel load failed: {}", e))?;

    Ok(result)
}

/// Save product metadata (upsert)
pub fn save(conn: &mut diesel::SqliteConnection, meta: &ProductMetadata) -> Result<()> {
    use crate::diesel_schema::product_metadata::dsl::*;

    diesel::insert_into(product_metadata)
        .values(meta)
        .on_conflict(id)
        .do_update()
        .set(meta)
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel save failed: {}", e))?;

    Ok(())
}

/// Get product by external ID
pub fn get_by_external_id(
    conn: &mut diesel::SqliteConnection,
    source: MetadataSource,
    external_id: &str,
) -> Result<Option<ProductMetadata>> {
    let id = format!("{}:{}", source.as_str(), external_id);
    load(conn, &id)
}

/// List all products from a specific source
pub fn list_by_source(
    conn: &mut diesel::SqliteConnection,
    src: MetadataSource,
) -> Result<Vec<ProductMetadata>> {
    use crate::diesel_schema::product_metadata::dsl::*;

    let results = product_metadata
        .filter(source.eq(src.as_str()))
        .select(ProductMetadata::as_select())
        .load(conn)
        .map_err(|e| anyhow::anyhow!("Diesel list failed: {}", e))?;

    Ok(results)
}
