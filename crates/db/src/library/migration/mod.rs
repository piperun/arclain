//! Schema migration from arclain's old product_metadata to gameta's format.
//!
//! Detects the old schema (presence of `genres_json` column) and converts:
//! - Column renames: genres_json → genres, tags_json → tags, languages_json → languages
//! - DLSite-specific top-level fields merged into `extras` JSON
//! - Timestamp conversion: ISO 8601 strings → Unix timestamps
//! - geo_blocked: Option<bool> → bool (integer 0/1)
//! - Source normalization: "itch" → "itchio"
//! - Drops: last_accessed, product_formats_json (moved to extras)
//!
//! Per-row shaping (legacy columns → gameta columns, extras JSON
//! merging, timestamp parsing) lives in [`convert_row`]; this file is
//! just the migration lifecycle.

mod convert_row;

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

use convert_row::{convert_and_insert, read_old_rows};

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

    // Back up the database file before the destructive DROP TABLE. If the
    // backup fails we abort the migration — the user can fix the cause
    // (full disk, perms, stale .bak path) and retry.
    if let Some(path) = db_path {
        let backup_path = path.with_extension("sqlite.bak");
        std::fs::copy(path, &backup_path).with_context(|| {
            format!(
                "Failed to create migration backup at {:?}; refusing to proceed with destructive schema migration",
                backup_path,
            )
        })?;
        tracing::info!("[Migration] Backup created at {}", backup_path.display());
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

    /// Regression test for C4 from `docs/AUDIT_2026-05-03.md`.
    ///
    /// Pre-fix, `migrate_to_gameta_schema` only logged a warning when the
    /// `.sqlite.bak` backup failed and proceeded with the destructive
    /// `DROP TABLE` — violating "never destroy without verified backup."
    /// Post-fix, a backup failure aborts the migration so the user can
    /// fix the cause (full disk, perms, stale .bak path) and retry.
    ///
    /// Force the backup to fail by pre-creating `<db>.sqlite.bak` as a
    /// directory — `std::fs::copy` fails cross-platform when the
    /// destination is a directory. Assert the function returns `Err`,
    /// the backup directory is untouched, and the original schema is
    /// preserved (the destructive DROP TABLE didn't run).
    #[test]
    fn c4_migration_aborts_on_backup_failure() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("library.sqlite");
        let backup_path = temp.path().join("library.sqlite.bak");

        // Make the backup destination be a directory: fs::copy(file, dir)
        // fails on every supported platform.
        std::fs::create_dir(&backup_path).unwrap();

        // Set up an old-schema DB with one row.
        let conn = Connection::open(&db_path).unwrap();
        create_old_schema(&conn);
        conn.execute(
            "INSERT INTO product_metadata (
                id, source, external_id, title, geo_blocked, cached_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "dlsite:RJ12345",
                "dlsite",
                "RJ12345",
                "Test",
                0,
                "2024-01-01T00:00:00Z",
            ],
        )
        .unwrap();

        assert!(has_old_schema(&conn).unwrap());

        let result = migrate_to_gameta_schema(&conn, Some(&db_path));

        assert!(
            result.is_err(),
            "C4 fix regressed: migration returned Ok despite backup failing",
        );

        // The .bak directory should be untouched.
        assert!(
            backup_path.is_dir(),
            "Sanity: backup destination should still be a directory",
        );

        // And the original schema is still in place — destructive ops
        // never ran.
        assert!(
            has_old_schema(&conn).unwrap(),
            "C4 fix regressed: old schema was destroyed even though backup failed",
        );
    }
}
