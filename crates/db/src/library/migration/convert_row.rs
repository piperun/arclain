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
use rusqlite::{Connection, Row};

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
        "SELECT rowid AS legacy_rowid,
                id, source, external_id, title, creator, description, release_date,
                price, currency, rating, rating_count, purchase_count, favorite_count, review_count,
                file_size, file_format, age_rating,
                genres_json, tags_json, languages_json, product_formats_json,
                series_name, illustrator, voice_actors_json, miscellaneous, update_info, rankings_json,
                extras_json, raw_api_response, raw_html, geo_blocked, cached_at, updated_at
         FROM product_metadata
         ORDER BY rowid",
    )?;

    let mut rows = stmt.query([])?;
    let mut result = Vec::new();
    let mut ordinal = 0_usize;
    loop {
        let next_ordinal = ordinal + 1;
        let Some(row) = rows
            .next()
            .with_context(|| format!("step legacy product_metadata row ordinal {next_ordinal}"))?
        else {
            break;
        };
        ordinal = next_ordinal;

        let legacy_rowid: i64 = row
            .get("legacy_rowid")
            .with_context(|| format!("read legacy product_metadata row ordinal {ordinal} rowid"))?;
        let identity = format!("legacy product_metadata rowid {legacy_rowid} (ordinal {ordinal})");
        let id: String = row
            .get("id")
            .with_context(|| format!("read {identity} id"))?;
        let old = read_old_row(row, id).with_context(|| format!("read {identity}"))?;
        result.push(old);
    }
    Ok(result)
}

fn read_old_row(row: &Row<'_>, id: String) -> rusqlite::Result<OldRow> {
    Ok(OldRow {
        id,
        source: row.get("source")?,
        external_id: row.get("external_id")?,
        title: row.get::<_, Option<String>>("title")?,
        creator: row.get::<_, Option<String>>("creator")?,
        description: row.get::<_, Option<String>>("description")?,
        release_date: row.get::<_, Option<String>>("release_date")?,
        price: row.get::<_, Option<i64>>("price")?,
        currency: row.get::<_, Option<String>>("currency")?,
        rating: row.get::<_, Option<f64>>("rating")?,
        rating_count: row.get::<_, Option<i64>>("rating_count")?,
        purchase_count: row.get::<_, Option<i64>>("purchase_count")?,
        favorite_count: row.get::<_, Option<i64>>("favorite_count")?,
        review_count: row.get::<_, Option<i64>>("review_count")?,
        file_size: row.get::<_, Option<String>>("file_size")?,
        file_format: row.get::<_, Option<String>>("file_format")?,
        age_rating: row.get::<_, Option<String>>("age_rating")?,
        genres_json: row.get::<_, Option<String>>("genres_json")?,
        tags_json: row.get::<_, Option<String>>("tags_json")?,
        languages_json: row.get::<_, Option<String>>("languages_json")?,
        product_formats_json: row.get::<_, Option<String>>("product_formats_json")?,
        series_name: row.get::<_, Option<String>>("series_name")?,
        illustrator: row.get::<_, Option<String>>("illustrator")?,
        voice_actors_json: row.get::<_, Option<String>>("voice_actors_json")?,
        miscellaneous: row.get::<_, Option<String>>("miscellaneous")?,
        update_info: row.get::<_, Option<String>>("update_info")?,
        rankings_json: row.get::<_, Option<String>>("rankings_json")?,
        extras_json: row.get::<_, Option<String>>("extras_json")?,
        raw_api_response: row.get::<_, Option<String>>("raw_api_response")?,
        raw_html: row.get::<_, Option<String>>("raw_html")?,
        geo_blocked: row.get::<_, Option<bool>>("geo_blocked")?,
        cached_at: row.get("cached_at")?,
        updated_at: row.get::<_, Option<String>>("updated_at")?,
    })
}

pub(super) fn convert_and_insert(conn: &Connection, old: &OldRow) -> Result<()> {
    validate_json(&old.genres_json, "genres_json", &old.id)?;
    validate_json(&old.tags_json, "tags_json", &old.id)?;
    validate_json(&old.languages_json, "languages_json", &old.id)?;
    validate_json(&old.product_formats_json, "product_formats_json", &old.id)?;
    validate_json(&old.voice_actors_json, "voice_actors_json", &old.id)?;
    validate_json(&old.rankings_json, "rankings_json", &old.id)?;
    validate_json(&old.extras_json, "extras_json", &old.id)?;

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
    let extras = build_extras(old)?;

    // Convert geo_blocked: Option<bool> → integer 0/1
    let geo_blocked: i32 = old.geo_blocked.unwrap_or(false) as i32;

    // Convert timestamps: ISO 8601 → Unix timestamp
    let cached_at = required_timestamp(&old.cached_at, "cached_at", &old.id)?;
    let updated_at = old
        .updated_at
        .as_deref()
        .map(|value| required_timestamp(value, "updated_at", &old.id))
        .transpose()?;

    conn.execute(
        "INSERT INTO product_metadata (
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
fn build_extras(old: &OldRow) -> Result<Option<String>> {
    let mut extras: serde_json::Map<String, serde_json::Value> = match &old.extras_json {
        Some(value) => serde_json::from_str(value)
            .with_context(|| format!("row {:?} has invalid extras_json object", old.id))?,
        None => serde_json::Map::new(),
    };

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
        let value = serde_json::from_str(v)
            .with_context(|| format!("row {:?} has invalid voice_actors_json", old.id))?;
        extras.insert("voice_actors".to_string(), value);
    }
    if let Some(ref v) = old.product_formats_json {
        let value = serde_json::from_str(v)
            .with_context(|| format!("row {:?} has invalid product_formats_json", old.id))?;
        extras.insert("product_formats".to_string(), value);
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
        let value = serde_json::from_str(v)
            .with_context(|| format!("row {:?} has invalid rankings_json", old.id))?;
        extras.insert("rankings".to_string(), value);
    }

    if extras.is_empty() {
        Ok(None)
    } else {
        serde_json::to_string(&extras)
            .with_context(|| format!("serialize converted extras for row {:?}", old.id))
            .map(Some)
    }
}

fn validate_json(value: &Option<String>, field: &str, row_id: &str) -> Result<()> {
    if let Some(value) = value {
        serde_json::from_str::<serde_json::Value>(value)
            .with_context(|| format!("row {row_id:?} has invalid {field}"))?;
    }
    Ok(())
}

fn required_timestamp(value: &str, field: &str, row_id: &str) -> Result<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("row {row_id:?} has invalid {field}"))
        .map(|date| date.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_required_timestamp() {
        assert_eq!(
            required_timestamp("2024-01-01T00:00:00+00:00", "cached_at", "row").unwrap(),
            1704067200
        );
        assert_eq!(
            required_timestamp("2024-01-01T00:00:00Z", "cached_at", "row").unwrap(),
            1704067200
        );
        assert!(required_timestamp("invalid", "cached_at", "row").is_err());
    }
}
