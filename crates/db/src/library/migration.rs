//! Schema migration from arclain's old product_metadata to gameta's format.
//!
//! Detects the old schema (presence of `genres_json` column) and converts:
//! - Column renames: genres_json → genres, tags_json → tags, languages_json → languages
//! - DLSite-specific top-level fields merged into `extras` JSON
//! - Timestamp conversion: ISO 8601 strings → Unix timestamps
//! - geo_blocked: Option<bool> → bool (integer 0/1)
//! - Source normalization: "itch" → "itchio"
//! - Drops: last_accessed, product_formats_json (moved to extras)

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

/// Result of the migration attempt
pub enum MigrationResult {
    /// No migration needed (already new schema or empty db)
    NotNeeded,
    /// Migration completed successfully
    Migrated { total: usize, converted: usize },
}

/// Check if the database has the old arclain schema.
/// Returns true if `genres_json` column exists in product_metadata.
fn has_old_schema(conn: &Connection) -> Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_info(product_metadata)")?;
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();

    if columns.is_empty() {
        // Table doesn't exist yet
        return Ok(false);
    }

    // Old schema has `genres_json`, new schema has `genres`
    Ok(columns.iter().any(|c| c == "genres_json"))
}

/// Migrate from arclain's old product_metadata schema to gameta's schema.
///
/// If `db_path` is provided, creates a `.sqlite.bak` backup before migrating.
/// Uses only rusqlite since the schema is in flux between old and new formats.
pub fn migrate_to_gameta_schema(
    conn: &Connection,
    db_path: Option<&Path>,
) -> Result<MigrationResult> {
    if !has_old_schema(conn)? {
        return Ok(MigrationResult::NotNeeded);
    }

    tracing::info!("[Migration] Old arclain schema detected, migrating to gameta format...");

    // Back up the database file
    if let Some(path) = db_path {
        let backup_path = path.with_extension("sqlite.bak");
        match std::fs::copy(path, &backup_path) {
            Ok(_) => tracing::info!("[Migration] Backup created at {:?}", backup_path),
            Err(e) => tracing::warn!(
                "[Migration] Failed to create backup at {:?}: {}",
                backup_path,
                e
            ),
        }
    }

    // Read all rows from old table
    let old_rows = read_old_rows(conn)?;
    let total = old_rows.len();
    tracing::info!("[Migration] Read {} rows from old schema", total);

    // Drop old table, indexes, and any leftover cr-sqlite triggers
    // (cr-sqlite was removed but its triggers persist in existing databases)
    let triggers: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='trigger'")?
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    for trigger in &triggers {
        let _ = conn.execute_batch(&format!("DROP TRIGGER IF EXISTS \"{}\"", trigger));
    }
    conn.execute_batch(
        "DROP TABLE IF EXISTS product_metadata;
         DROP INDEX IF EXISTS idx_product_source;
         DROP INDEX IF EXISTS idx_product_external;",
    )?;

    // Create new table with gameta schema
    conn.execute_batch(NEW_SCHEMA_SQL)?;

    // Convert and insert each row
    let mut converted = 0;
    for row in &old_rows {
        match convert_and_insert(conn, row) {
            Ok(()) => converted += 1,
            Err(e) => {
                tracing::warn!("[Migration] Failed to convert row '{}': {}", row.id, e);
            }
        }
    }

    tracing::info!(
        "[Migration] Complete: {}/{} rows migrated",
        converted,
        total
    );

    Ok(MigrationResult::Migrated { total, converted })
}

/// Ensure the gameta product_metadata table exists.
/// Safe to call even if the table already exists (uses CREATE IF NOT EXISTS).
pub fn ensure_gameta_product_metadata_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(NEW_SCHEMA_SQL)?;
    Ok(())
}

/// New product_metadata table SQL (matches gameta_database schema)
const NEW_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS product_metadata (
    id TEXT NOT NULL PRIMARY KEY,
    source TEXT NOT NULL DEFAULT '',
    external_id TEXT NOT NULL DEFAULT '',
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
    genres TEXT,
    tags TEXT,
    languages TEXT,
    extras TEXT,
    raw_api_response TEXT,
    raw_html TEXT,
    geo_blocked INTEGER DEFAULT 0,
    cached_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_product_metadata_external
    ON product_metadata(source, external_id);
";

/// Intermediate struct for reading old arclain rows
struct OldRow {
    id: String,
    source: String,
    external_id: String,
    title: Option<String>,
    creator: Option<String>,
    description: Option<String>,
    release_date: Option<String>,
    price: Option<i64>,
    currency: Option<String>,
    rating: Option<f64>,
    rating_count: Option<i64>,
    purchase_count: Option<i64>,
    favorite_count: Option<i64>,
    review_count: Option<i64>,
    file_size: Option<String>,
    file_format: Option<String>,
    age_rating: Option<String>,
    genres_json: Option<String>,
    tags_json: Option<String>,
    languages_json: Option<String>,
    product_formats_json: Option<String>,
    series_name: Option<String>,
    illustrator: Option<String>,
    voice_actors_json: Option<String>,
    miscellaneous: Option<String>,
    update_info: Option<String>,
    rankings_json: Option<String>,
    extras_json: Option<String>,
    raw_api_response: Option<String>,
    raw_html: Option<String>,
    geo_blocked: Option<bool>,
    cached_at: String,
    updated_at: Option<String>,
}

fn read_old_rows(conn: &Connection) -> Result<Vec<OldRow>> {
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

fn convert_and_insert(conn: &Connection, old: &OldRow) -> Result<()> {
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

    fn create_old_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE product_metadata (
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
            );",
        )
        .unwrap();
    }

    #[test]
    fn test_detects_old_schema() {
        let conn = Connection::open_in_memory().unwrap();
        create_old_schema(&conn);
        assert!(has_old_schema(&conn).unwrap());
    }

    #[test]
    fn test_detects_new_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(NEW_SCHEMA_SQL).unwrap();
        assert!(!has_old_schema(&conn).unwrap());
    }

    #[test]
    fn test_no_table_is_not_old() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(!has_old_schema(&conn).unwrap());
    }

    #[test]
    fn test_migration_not_needed() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(NEW_SCHEMA_SQL).unwrap();

        let result = migrate_to_gameta_schema(&conn, None).unwrap();
        assert!(matches!(result, MigrationResult::NotNeeded));
    }

    #[test]
    fn test_migration_converts_rows() {
        let conn = Connection::open_in_memory().unwrap();
        create_old_schema(&conn);

        // Insert a test row with old schema
        conn.execute(
            "INSERT INTO product_metadata (
                id, source, external_id, title, creator,
                genres_json, tags_json, languages_json,
                series_name, illustrator, voice_actors_json,
                geo_blocked, cached_at, updated_at
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            rusqlite::params![
                "dlsite:RJ100200",
                "dlsite",
                "RJ100200",
                "Test Product",
                "Test Circle",
                r#"["RPG","Adventure"]"#,
                r#"["tag1","tag2"]"#,
                r#"["Japanese","English"]"#,
                "Test Series",
                "Test Illustrator",
                r#"["Actor1","Actor2"]"#,
                1, // geo_blocked = true
                "2024-06-15T12:30:00+00:00",
                "2024-07-01T08:00:00+00:00",
            ],
        )
        .unwrap();

        let result = migrate_to_gameta_schema(&conn, None).unwrap();
        assert!(matches!(
            result,
            MigrationResult::Migrated {
                total: 1,
                converted: 1
            }
        ));

        // Verify new schema columns exist
        assert!(!has_old_schema(&conn).unwrap());

        // Verify data was converted
        let mut stmt = conn
            .prepare(
                "SELECT id, genres, tags, languages, extras, geo_blocked, cached_at, updated_at
                 FROM product_metadata WHERE id = ?1",
            )
            .unwrap();

        let row = stmt
            .query_row(["dlsite:RJ100200"], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i32>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                ))
            })
            .unwrap();

        assert_eq!(row.0, "dlsite:RJ100200");
        assert_eq!(row.1.as_deref(), Some(r#"["RPG","Adventure"]"#));
        assert_eq!(row.2.as_deref(), Some(r#"["tag1","tag2"]"#));
        assert_eq!(row.3.as_deref(), Some(r#"["Japanese","English"]"#));

        // Verify extras merged DLSite fields
        let extras: serde_json::Value = serde_json::from_str(row.4.as_ref().unwrap()).unwrap();
        assert_eq!(extras["series_name"], "Test Series");
        assert_eq!(extras["illustrator"], "Test Illustrator");
        assert_eq!(extras["voice_actors"], serde_json::json!(["Actor1", "Actor2"]));

        // geo_blocked converted to integer
        assert_eq!(row.5, 1);

        // Timestamps converted to Unix
        assert_eq!(row.6, 1718454600); // 2024-06-15T12:30:00Z
        assert_eq!(row.7, Some(1719820800)); // 2024-07-01T08:00:00Z
    }

    #[test]
    fn test_migration_normalizes_itch_source() {
        let conn = Connection::open_in_memory().unwrap();
        create_old_schema(&conn);

        conn.execute(
            "INSERT INTO product_metadata (id, source, external_id, cached_at)
             VALUES ('itch:12345', 'itch', '12345', '2024-01-01T00:00:00+00:00')",
            [],
        )
        .unwrap();

        migrate_to_gameta_schema(&conn, None).unwrap();

        // Verify source and id were normalized
        let (id, source): (String, String) = conn
            .query_row(
                "SELECT id, source FROM product_metadata LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(id, "itchio:12345");
        assert_eq!(source, "itchio");
    }

    #[test]
    fn test_migration_handles_null_geo_blocked() {
        let conn = Connection::open_in_memory().unwrap();
        create_old_schema(&conn);

        conn.execute(
            "INSERT INTO product_metadata (id, source, external_id, geo_blocked, cached_at)
             VALUES ('dlsite:RJ999', 'dlsite', 'RJ999', NULL, '2024-01-01T00:00:00+00:00')",
            [],
        )
        .unwrap();

        migrate_to_gameta_schema(&conn, None).unwrap();

        let geo_blocked: i32 = conn
            .query_row(
                "SELECT geo_blocked FROM product_metadata WHERE id = 'dlsite:RJ999'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(geo_blocked, 0); // NULL → false → 0
    }

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
