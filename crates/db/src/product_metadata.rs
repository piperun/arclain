//! Unified product metadata stored in the database.
//!
//! This stores metadata from any source (DLSite, itch.io, Steam, etc.) in a single table.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Row};

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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

    /// Parse from a database row
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            source: row.get(1)?,
            external_id: row.get(2)?,
            title: row.get(3).ok(),
            creator: row.get(4).ok(),
            description: row.get(5).ok(),
            release_date: row.get(6).ok(),
            price: row.get(7).ok(),
            currency: row.get(8).ok(),
            rating: row.get(9).ok(),
            rating_count: row.get(10).ok(),
            purchase_count: row.get(11).ok(),
            favorite_count: row.get(12).ok(),
            review_count: row.get(13).ok(),
            file_size: row.get(14).ok(),
            file_format: row.get(15).ok(),
            age_rating: row.get(16).ok(),
            genres_json: row.get(17).ok(),
            tags_json: row.get(18).ok(),
            languages_json: row.get(19).ok(),
            product_formats_json: row.get(20).ok(),
            series_name: row.get(21).ok(),
            illustrator: row.get(22).ok(),
            voice_actors_json: row.get(23).ok(),
            miscellaneous: row.get(24).ok(),
            update_info: row.get(25).ok(),
            rankings_json: row.get(26).ok(),
            extras_json: row.get(27).ok(),
            raw_api_response: row.get(28).ok(),
            raw_html: row.get(29).ok(),
            cached_at: row.get(30)?,
            updated_at: row.get(31).ok(),
            last_accessed: row.get(32).ok(),
        })
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
    cached_at TEXT NOT NULL,
    updated_at TEXT,
    last_accessed TEXT
);
CREATE INDEX IF NOT EXISTS idx_product_source ON product_metadata(source);
CREATE INDEX IF NOT EXISTS idx_product_external ON product_metadata(external_id);
"#;

const SELECT_COLS: &str = "id, source, external_id, title, creator, description, release_date, \
    price, currency, rating, rating_count, purchase_count, favorite_count, review_count, \
    file_size, file_format, age_rating, genres_json, tags_json, languages_json, product_formats_json, \
    series_name, illustrator, voice_actors_json, miscellaneous, update_info, rankings_json, \
    extras_json, raw_api_response, raw_html, cached_at, updated_at, last_accessed";

/// Initialize the product_metadata table
pub fn init_product_metadata_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(CREATE_TABLE_SQL)?;
    Ok(())
}

/// Load product metadata by ID
pub fn load(conn: &Connection, id: &str) -> Result<Option<ProductMetadata>> {
    let sql = format!("SELECT {} FROM product_metadata WHERE id = ?1", SELECT_COLS);
    let mut stmt = conn.prepare(&sql)?;
    let entry = stmt.query_row([id], ProductMetadata::from_row).optional()?;
    Ok(entry)
}

/// Save product metadata (upsert)
pub fn save(conn: &Connection, m: &ProductMetadata) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO product_metadata (
            id, source, external_id, title, creator, description, release_date,
            price, currency, rating, rating_count, purchase_count, favorite_count, review_count,
            file_size, file_format, age_rating, genres_json, tags_json, languages_json, product_formats_json,
            series_name, illustrator, voice_actors_json, miscellaneous, update_info, rankings_json,
            extras_json, raw_api_response, raw_html, cached_at, updated_at, last_accessed
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33)",
        params![
            &m.id, &m.source, &m.external_id, &m.title, &m.creator, &m.description, &m.release_date,
            &m.price, &m.currency, &m.rating, &m.rating_count, &m.purchase_count, &m.favorite_count, &m.review_count,
            &m.file_size, &m.file_format, &m.age_rating, &m.genres_json, &m.tags_json, &m.languages_json, &m.product_formats_json,
            &m.series_name, &m.illustrator, &m.voice_actors_json, &m.miscellaneous, &m.update_info, &m.rankings_json,
            &m.extras_json, &m.raw_api_response, &m.raw_html, &m.cached_at, &m.updated_at, &m.last_accessed
        ],
    )?;
    Ok(())
}

/// Get product by external ID
pub fn get_by_external_id(
    conn: &Connection,
    source: MetadataSource,
    external_id: &str,
) -> Result<Option<ProductMetadata>> {
    let id = format!("{}:{}", source.as_str(), external_id);
    load(conn, &id)
}

/// List all products from a specific source
pub fn list_by_source(conn: &Connection, source: MetadataSource) -> Result<Vec<ProductMetadata>> {
    let sql = format!(
        "SELECT {} FROM product_metadata WHERE source = ?1",
        SELECT_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([source.as_str()], ProductMetadata::from_row)?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Delete product metadata by ID
pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM product_metadata WHERE id = ?1", [id])?;
    Ok(())
}
