//! Row-conversion helpers for the old → gameta migration.
//!
//! Reads the legacy arclain `product_metadata` rows, normalizes them
//! into the gameta-shape (column renames, source aliasing, ISO →
//! Unix-timestamp dates, DLsite-specific fields folded into `extras`
//! JSON), and writes them out again.
//!
//! Lifted out of `migration/mod.rs` so the orchestration file
//! (`migrate_to_gameta_schema`, schema-existence checks, the new
//! schema DDL) stays focused on the migration lifecycle and isn't
//! buried under 200 lines of column-by-column shaping.

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Intermediate struct for reading old arclain rows
pub(super) struct OldRow {
    pub id: String,
    pub source: String,
    pub external_id: String,
    pub title: Option<String>,
    pub creator: Option<String>,
    pub description: Option<String>,
    pub release_date: Option<String>,
    pub price: Option<i64>,
    pub currency: Option<String>,
    pub rating: Option<f64>,
    pub rating_count: Option<i64>,
    pub purchase_count: Option<i64>,
    pub favorite_count: Option<i64>,
    pub review_count: Option<i64>,
    pub file_size: Option<String>,
    pub file_format: Option<String>,
    pub age_rating: Option<String>,
    pub genres_json: Option<String>,
    pub tags_json: Option<String>,
    pub languages_json: Option<String>,
    pub product_formats_json: Option<String>,
    pub series_name: Option<String>,
    pub illustrator: Option<String>,
    pub voice_actors_json: Option<String>,
    pub miscellaneous: Option<String>,
    pub update_info: Option<String>,
    pub rankings_json: Option<String>,
    pub extras_json: Option<String>,
    pub raw_api_response: Option<String>,
    pub raw_html: Option<String>,
    pub geo_blocked: Option<bool>,
    pub cached_at: String,
    pub updated_at: Option<String>,
}

pub(super) fn read_old_rows(conn: &Connection) -> Result<Vec<OldRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, source, external_id, title, creator, description, release_date,
                price, currency, rating, rating_count, purchase_count, favorite_count, review_count,
                file_size, file_format, age_rating,
                genres_json, tags_json, languages_json, product_formats_json,
                series_name, illustrator, voice_actors_json, miscellaneous, update_info, rankings_json,
                extras_json, raw_api_response, raw_html, geo_blocked, cached_at, updated_at
         FROM product_metadata",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(OldRow {
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
            geo_blocked: row.get(30).ok(),
            cached_at: row.get(31)?,
            updated_at: row.get(32).ok(),
        })
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row.context("Failed to read old row")?);
    }
    Ok(result)
}

pub(super) fn convert_and_insert(conn: &Connection, old: &OldRow) -> Result<()> {
    // Normalize source: "itch" → "itchio" (gameta convention)
    let source = normalize_source(&old.source);

    // Normalize ID if source changed
    let id = if source != old.source {
        format!("{}:{}", source, old.external_id)
    } else {
        old.id.clone()
    };

    // JSON arrays pass through as-is (already valid JSON strings)
    let genres = &old.genres_json;
    let tags = &old.tags_json;
    let languages = &old.languages_json;

    // Merge DLSite-specific fields into extras
    let extras = build_extras(old);

    // Convert geo_blocked: Option<bool> → integer 0/1
    let geo_blocked: i32 = old.geo_blocked.unwrap_or(false) as i32;

    // Convert timestamps: ISO 8601 → Unix timestamp
    let cached_at = parse_iso_to_unix(&old.cached_at).unwrap_or(0);
    let updated_at = old.updated_at.as_ref().and_then(|s| parse_iso_to_unix(s));

    conn.execute(
        "INSERT OR REPLACE INTO product_metadata (
            id, source, external_id, title, creator, description, release_date,
            price, currency, rating, rating_count, purchase_count, favorite_count, review_count,
            file_size, file_format, age_rating, genres, tags, languages,
            extras, raw_api_response, raw_html, geo_blocked, cached_at, updated_at
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26)",
        rusqlite::params![
            id,
            source,
            old.external_id,
            old.title,
            old.creator,
            old.description,
            old.release_date,
            old.price,
            old.currency,
            old.rating,
            old.rating_count,
            old.purchase_count,
            old.favorite_count,
            old.review_count,
            old.file_size,
            old.file_format,
            old.age_rating,
            genres,
            tags,
            languages,
            extras,
            old.raw_api_response,
            old.raw_html,
            geo_blocked,
            cached_at,
            updated_at,
        ],
    )?;

    Ok(())
}

/// Normalize source strings to gameta convention
fn normalize_source(source: &str) -> String {
    match source.to_lowercase().as_str() {
        "itch" => "itchio".to_string(),
        other => other.to_string(),
    }
}

/// Build the extras JSON by merging old extras_json with DLSite-specific fields
fn build_extras(old: &OldRow) -> Option<String> {
    let mut extras: serde_json::Map<String, serde_json::Value> = old
        .extras_json
        .as_ref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    // Merge DLSite-specific fields into extras
    if let Some(ref v) = old.series_name {
        extras.insert(
            "series_name".to_string(),
            serde_json::Value::String(v.clone()),
        );
    }
    if let Some(ref v) = old.illustrator {
        extras.insert(
            "illustrator".to_string(),
            serde_json::Value::String(v.clone()),
        );
    }
    if let Some(ref v) = old.voice_actors_json {
        if let Ok(arr) = serde_json::from_str::<serde_json::Value>(v) {
            extras.insert("voice_actors".to_string(), arr);
        }
    }
    if let Some(ref v) = old.product_formats_json {
        if let Ok(arr) = serde_json::from_str::<serde_json::Value>(v) {
            extras.insert("product_formats".to_string(), arr);
        }
    }
    if let Some(ref v) = old.miscellaneous {
        extras.insert(
            "miscellaneous".to_string(),
            serde_json::Value::String(v.clone()),
        );
    }
    if let Some(ref v) = old.update_info {
        extras.insert(
            "update_info".to_string(),
            serde_json::Value::String(v.clone()),
        );
    }
    if let Some(ref v) = old.rankings_json {
        if let Ok(arr) = serde_json::from_str::<serde_json::Value>(v) {
            extras.insert("rankings".to_string(), arr);
        }
    }

    if extras.is_empty() {
        None
    } else {
        serde_json::to_string(&extras).ok()
    }
}

/// Parse ISO 8601 / RFC 3339 string to Unix timestamp
fn parse_iso_to_unix(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_iso_to_unix() {
        assert_eq!(
            parse_iso_to_unix("2024-01-01T00:00:00+00:00"),
            Some(1704067200)
        );
        assert_eq!(
            parse_iso_to_unix("2024-01-01T00:00:00Z"),
            Some(1704067200)
        );
        assert_eq!(parse_iso_to_unix("invalid"), None);
    }
}
