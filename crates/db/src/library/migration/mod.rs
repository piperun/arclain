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
use std::path::{Path, PathBuf};
use std::time::Duration;

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
    let mut stmt = conn
        .prepare("PRAGMA table_info(product_metadata)")
        .context("inspect product_metadata schema")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("read product_metadata schema columns")?;

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

    // Back up the live database before destructive work. SQLite's online
    // backup API includes committed WAL frames that a raw file copy misses.
    if let Some(path) = db_path {
        let backup_path = backup_database(conn, path)?;
        tracing::info!("[Migration] Backup created at {}", backup_path.display());
    }

    // Read all rows from old table
    let old_rows = read_old_rows(conn)?;
    let total = old_rows.len();
    tracing::info!("[Migration] Read {} rows from old schema", total);

    let transaction = conn
        .unchecked_transaction()
        .context("begin library schema migration transaction")?;

    // Drop old table, indexes, and any leftover cr-sqlite triggers
    // (cr-sqlite was removed but its triggers persist in existing databases).
    let triggers: Vec<String> = {
        let mut statement = transaction
            .prepare("SELECT name FROM sqlite_master WHERE type='trigger'")
            .context("enumerate legacy product metadata triggers")?;
        let triggers = statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("read legacy product metadata trigger names")?;
        triggers
    };
    for trigger in &triggers {
        let quoted_trigger = trigger.replace('"', "\"\"");
        transaction
            .execute_batch(&format!("DROP TRIGGER IF EXISTS \"{quoted_trigger}\""))
            .with_context(|| format!("drop legacy trigger {trigger:?}"))?;
    }
    transaction
        .execute_batch(
            "DROP TABLE IF EXISTS product_metadata;
             DROP INDEX IF EXISTS idx_product_source;
             DROP INDEX IF EXISTS idx_product_external;",
        )
        .context("drop legacy product metadata schema")?;

    // Create new table with gameta schema
    transaction
        .execute_batch(NEW_SCHEMA_SQL)
        .context("create gameta product metadata schema")?;

    // Conversion, destructive DDL, and every insert commit as one unit.
    for row in &old_rows {
        convert_and_insert(&transaction, row)
            .with_context(|| format!("convert legacy product_metadata row {:?}", row.id))?;
    }
    transaction
        .commit()
        .context("commit library schema migration")?;

    tracing::info!("[Migration] Complete: {}/{} rows migrated", total, total);

    Ok(MigrationResult::Migrated {
        total,
        converted: total,
    })
}

fn backup_database(conn: &Connection, db_path: &Path) -> Result<PathBuf> {
    use rusqlite::backup::Backup;

    let backup_path = db_path.with_extension("sqlite.bak");
    let mut destination = Connection::open(&backup_path)
        .with_context(|| format!("open migration backup {}", backup_path.display()))?;
    let backup = Backup::new(conn, &mut destination).context("initialize SQLite online backup")?;
    backup
        .run_to_completion(64, Duration::from_millis(10), None)
        .context("copy live SQLite database through backup API")?;
    drop(backup);

    let integrity: String = destination
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .context("verify migration backup")?;
    if integrity != "ok" {
        anyhow::bail!("migration backup integrity check failed: {integrity}");
    }

    Ok(backup_path)
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
        assert_eq!(
            extras["voice_actors"],
            serde_json::json!(["Actor1", "Actor2"])
        );

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
    fn normalized_id_collision_aborts_and_rolls_back_the_entire_migration() {
        let conn = Connection::open_in_memory().unwrap();
        create_old_schema(&conn);
        conn.execute(
            "INSERT INTO product_metadata (id, source, external_id, cached_at) \
             VALUES ('itch:12345', 'itch', '12345', '2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO product_metadata (id, source, external_id, cached_at) \
             VALUES ('itchio:12345', 'itchio', '12345', '2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let error = match migrate_to_gameta_schema(&conn, None) {
            Ok(_) => panic!("migration silently replaced a normalized-ID collision"),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains("itchio:12345"),
            "colliding row id missing from error: {error}"
        );
        assert!(
            has_old_schema(&conn).unwrap(),
            "legacy schema was not rolled back after normalized-ID collision"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM product_metadata", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 2, "migration lost a colliding legacy row");
    }

    #[test]
    fn invalid_id_read_error_includes_legacy_rowid() {
        let conn = Connection::open_in_memory().unwrap();
        create_old_schema(&conn);
        conn.execute(
            "INSERT INTO product_metadata (id, source, external_id, cached_at) \
             VALUES (NULL, 'dlsite', 'NULL-ID', '2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let legacy_rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM product_metadata WHERE id IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let error = match migrate_to_gameta_schema(&conn, None) {
            Ok(_) => panic!("migration accepted a NULL legacy id"),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains(&format!("rowid {legacy_rowid}")),
            "stable legacy row identity missing from error: {error}"
        );
        assert!(has_old_schema(&conn).unwrap());
    }

    #[test]
    fn migration_backup_contains_committed_wal_rows() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("library.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode, "wal", "test database did not enter WAL mode");
        create_old_schema(&conn);
        conn.execute(
            "INSERT INTO product_metadata (id, source, external_id, cached_at) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["dlsite:RJWAL", "dlsite", "RJWAL", "2024-01-01T00:00:00Z"],
        )
        .unwrap();
        let mut wal_path = db_path.as_os_str().to_os_string();
        wal_path.push("-wal");
        let wal_path = PathBuf::from(wal_path);
        let wal_len = std::fs::metadata(&wal_path)
            .expect("WAL file should exist while the live connection is open")
            .len();
        assert!(wal_len > 32, "WAL file contained no committed frames");

        migrate_to_gameta_schema(&conn, Some(&db_path)).unwrap();

        let backup = Connection::open(db_path.with_extension("sqlite.bak")).unwrap();
        let count: i64 = backup
            .query_row(
                "SELECT COUNT(*) FROM product_metadata WHERE id = 'dlsite:RJWAL'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "backup omitted a committed WAL row");
    }

    #[test]
    fn invalid_row_aborts_and_rolls_back_the_entire_migration() {
        let conn = Connection::open_in_memory().unwrap();
        create_old_schema(&conn);
        conn.execute(
            "INSERT INTO product_metadata (id, source, external_id, cached_at) \
             VALUES ('good', 'dlsite', 'GOOD', '2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO product_metadata (id, source, external_id, genres_json, cached_at) \
             VALUES ('bad', 'dlsite', 'BAD', '[not-json', 'not-a-timestamp')",
            [],
        )
        .unwrap();

        let error = match migrate_to_gameta_schema(&conn, None) {
            Ok(_) => panic!("migration accepted a malformed legacy row"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("bad"), "row id missing from error: {error}");
        assert!(
            has_old_schema(&conn).unwrap(),
            "legacy schema was not rolled back"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM product_metadata", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 2, "migration committed only a subset of rows");
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
    /// directory, which SQLite cannot open as a destination database. Assert
    /// the function returns `Err`,
    /// the backup directory is untouched, and the original schema is
    /// preserved (the destructive DROP TABLE didn't run).
    #[test]
    fn c4_migration_aborts_on_backup_failure() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("library.sqlite");
        let backup_path = temp.path().join("library.sqlite.bak");

        // Make the backup destination a directory so SQLite cannot open it.
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
