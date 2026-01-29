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

    /// Calculate a completeness score for data quality comparison.
    /// Higher score = more complete data.
    pub fn completeness_score(&self) -> u32 {
        let mut score: u32 = 0;

        // Core identity fields (high value)
        if self.title.is_some() {
            score += 10;
        }
        if self.creator.is_some() {
            score += 10;
        }

        // Description and content info (medium value)
        if self.description.is_some() {
            score += 5;
        }
        if self.release_date.is_some() {
            score += 2;
        }
        if self.price.is_some() {
            score += 1;
        }
        if self.file_size.is_some() {
            score += 1;
        }
        if self.file_format.is_some() {
            score += 1;
        }
        if self.age_rating.is_some() {
            score += 2;
        }

        // Ratings and stats (low value each, but useful)
        if self.rating.is_some() {
            score += 2;
        }
        if self.rating_count.is_some() {
            score += 1;
        }
        if self.purchase_count.is_some() {
            score += 1;
        }
        if self.favorite_count.is_some() {
            score += 1;
        }
        if self.review_count.is_some() {
            score += 1;
        }

        // DLSite-specific fields
        if self.series_name.is_some() {
            score += 2;
        }
        if self.illustrator.is_some() {
            score += 2;
        }
        if self.miscellaneous.is_some() {
            score += 1;
        }
        if self.update_info.is_some() {
            score += 1;
        }

        // JSON array fields - count actual elements
        score += Self::count_json_array_len(&self.genres_json) * 2; // 2 pts per genre
        score += Self::count_json_array_len(&self.tags_json); // 1 pt per tag
        score += Self::count_json_array_len(&self.languages_json); // 1 pt per language
        score += Self::count_json_array_len(&self.product_formats_json); // 1 pt per format
        score += Self::count_json_array_len(&self.voice_actors_json); // 1 pt per VA
        score += Self::count_json_array_len(&self.rankings_json); // 1 pt per ranking

        // Raw data storage (indicates full fetch)
        if self.raw_api_response.is_some() {
            score += 5;
        }
        if self.raw_html.is_some() {
            score += 3;
        }

        score
    }

    /// Count elements in a JSON array string
    fn count_json_array_len(json: &Option<String>) -> u32 {
        json.as_ref()
            .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(s).ok())
            .map(|arr| arr.len() as u32)
            .unwrap_or(0)
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

/// Delete product metadata by ID (cascades to cache_index)
pub fn delete(conn: &mut diesel::SqliteConnection, product_id: &str) -> Result<()> {
    use crate::diesel_schema::product_metadata::dsl::*;

    // Delete associated cache entries first (cascade)
    {
        use crate::diesel_schema::cache_index::dsl as cache_dsl;
        diesel::delete(cache_dsl::cache_index.filter(cache_dsl::product_id.eq(product_id)))
            .execute(conn)
            .map_err(|e| anyhow::anyhow!("Diesel cascade delete cache_index failed: {}", e))?;
    }

    // Delete the product metadata
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
///
/// Guards against data quality downgrade:
/// 1. If new data is geo-blocked and existing is NOT → reject
/// 2. If new data has lower completeness score than existing → reject
pub fn save(conn: &mut diesel::SqliteConnection, meta: &ProductMetadata) -> Result<()> {
    use crate::diesel_schema::product_metadata::dsl::*;

    // Check if we need to guard against overwriting better data
    if let Ok(Some(existing)) = load(conn, &meta.id) {
        let new_score = meta.completeness_score();
        let existing_score = existing.completeness_score();

        // Guard 1: Don't overwrite non-geo-blocked data with geo-blocked data
        if meta.geo_blocked == Some(true) && existing.geo_blocked != Some(true) {
            tracing::warn!(
                "[ProductMetadata] Skipping save for '{}' - refusing to overwrite good data (geo_blocked=false) with geo-blocked data",
                meta.id
            );
            return Ok(());
        }

        // Guard 2: Don't downgrade completeness score
        if new_score < existing_score {
            tracing::warn!(
                "[ProductMetadata] Skipping save for '{}' - new data less complete (score {} < {})",
                meta.id,
                new_score,
                existing_score
            );
            return Ok(());
        }

        tracing::debug!(
            "[ProductMetadata] Updating '{}' (score {} → {}, geo_blocked: {:?} → {:?})",
            meta.id,
            existing_score,
            new_score,
            existing.geo_blocked,
            meta.geo_blocked
        );
    } else {
        tracing::debug!(
            "[ProductMetadata] Inserting new '{}' (score {}, geo_blocked: {:?})",
            meta.id,
            meta.completeness_score(),
            meta.geo_blocked
        );
    }

    diesel::insert_into(product_metadata)
        .values(meta)
        .on_conflict(id)
        .do_update()
        .set(meta)
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel save failed: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completeness_score() {
        // 1. Basic empty metadata
        let mut m = ProductMetadata::new(MetadataSource::DLSite, "RJ100");
        let base_score = m.completeness_score();
        // Base score might vary if new() sets things, but expecting 0 for empty optional fields
        // Let's check increments relative to base

        // 2. Title (+10)
        m.title = Some("Test Title".to_string());
        assert_eq!(m.completeness_score(), base_score + 10);

        // 3. Creator (+10)
        m.creator = Some("Test Circle".to_string());
        assert_eq!(m.completeness_score(), base_score + 20);

        // 4. Description (+5 for len > 10)
        m.description = Some("A very long description...".to_string());
        assert_eq!(m.completeness_score(), base_score + 25);

        // 5. JSON arrays (Tags +1 per tag)
        // Note: set_tags logic might be complex, let's just set the json string directly if possible
        // But better to use the setter if available or manual json
        m.tags_json = Some("[\"Tag1\", \"Tag2\"]".to_string());
        // 2 tags * 1 point = +2
        assert_eq!(m.completeness_score(), base_score + 27);
    }
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

/// Migrate old entries that are missing extras_json.
/// Rebuilds extras_json from raw_api_response for DLSite entries.
/// Returns (checked_count, repaired_count).
pub fn migrate_repair_extras_json(conn: &mut diesel::SqliteConnection) -> Result<(usize, usize)> {
    use crate::diesel_schema::product_metadata::dsl::*;

    // Find DLSite entries where extras_json is NULL or doesn't contain screenshots
    let entries: Vec<ProductMetadata> = product_metadata
        .filter(source.eq("dlsite"))
        .select(ProductMetadata::as_select())
        .load(conn)
        .map_err(|e| anyhow::anyhow!("Diesel query failed: {}", e))?;

    let total = entries.len();
    let mut repaired = 0;

    for mut entry in entries {
        // Check if extras_json needs repair
        let needs_repair = entry
            .extras_json
            .as_ref()
            .map(|s| !s.contains("\"screenshots\":[\""))
            .unwrap_or(true);

        if !needs_repair {
            continue;
        }

        // Try to repair from raw_api_response
        if let Some(ref raw_json) = entry.raw_api_response {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw_json) {
                let cover_image = parsed["cover_image"].as_str();
                let screenshots: Vec<String> = parsed["sample_images"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                if cover_image.is_some() || !screenshots.is_empty() {
                    let extras = serde_json::json!({
                        "cover_image": cover_image,
                        "screenshots": screenshots,
                        "update_date": parsed["update_date"].as_str(),
                        "authors": parsed["authors"].as_array(),
                        "illustrators": parsed["illustrators"].as_array(),
                        "scenarios": parsed["scenarios"].as_array(),
                        "musicians": parsed["musicians"].as_array(),
                        "writers": parsed["writers"].as_array(),
                        "brand": parsed["brand"].as_str(),
                        "publisher": parsed["publisher"].as_str(),
                        "page_count": parsed["page_count"].as_i64(),
                    });
                    entry.extras_json = Some(extras.to_string());

                    // Direct update to avoid completeness score check blocking the repair
                    diesel::update(product_metadata.find(&entry.id))
                        .set(extras_json.eq(&entry.extras_json))
                        .execute(conn)
                        .map_err(|e| anyhow::anyhow!("Failed to update extras_json: {}", e))?;

                    repaired += 1;
                }
            }
        }
    }

    tracing::info!(
        "[Migration] Repaired extras_json: {}/{} entries updated",
        repaired,
        total
    );

    Ok((total, repaired))
}
