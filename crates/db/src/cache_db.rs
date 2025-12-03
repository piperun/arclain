use crate::sqlite_db::SqliteDb;
use anyhow::{Context, Result};
use std::path::Path;

/// Cache database for transient data like metadata cache
pub struct CacheDb {
    db: SqliteDb,
}

impl CacheDb {
    /// Open or create the cache database
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();

        // Try to open the database
        let db_result = SqliteDb::open(path_ref);

        // If opening failed and the file exists, it might be corrupt - try to delete and recreate
        if db_result.is_err() && path_ref.exists() {
            eprintln!("Warning: Cache database appears corrupt, attempting to recreate...");
            if let Err(e) = std::fs::remove_file(path_ref) {
                eprintln!("Failed to remove corrupt cache database: {}", e);
            } else {
                // Also try to remove WAL and SHM files
                let _ = std::fs::remove_file(path_ref.with_extension("sqlite-wal"));
                let _ = std::fs::remove_file(path_ref.with_extension("sqlite-shm"));
            }
        }

        // Try opening again (will create new file if we deleted the old one)
        let db = SqliteDb::open(path_ref)?;

        // Initialize cache schema
        db.init_schema(init_cache_schema)?;

        Ok(Self { db })
    }

    /// Get the underlying SqliteDb for sharing with other components
    pub fn into_sqlite_db(self) -> SqliteDb {
        self.db
    }

    /// Get a reference to the SqliteDb
    pub fn sqlite_db(&self) -> &SqliteDb {
        &self.db
    }
}

/// Initialize the cache database schema
fn init_cache_schema(conn: &rusqlite::Connection) -> Result<()> {
    // Create or update table with all DLSite fields
    conn.execute(
        "CREATE TABLE IF NOT EXISTS dlsite_metadata_cache (
            product_id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            circle TEXT,
            price INTEGER,
            release_date TEXT,
            description TEXT,
            work_type TEXT,
            file_format TEXT,
            tags_json TEXT,              -- JSON array of tags
            raw_api_json TEXT NOT NULL,   -- Original DLSite API response
            cached_at INTEGER NOT NULL
        )",
        [],
    )
    .context("Failed to create dlsite_metadata_cache table")?;

    // Add new columns if they don't exist (for migration from old schema)
    let add_column_if_missing = |col_name: &str, col_def: &str| -> Result<()> {
        let check_sql = format!(
            "SELECT COUNT(*) FROM pragma_table_info('dlsite_metadata_cache') WHERE name='{}'",
            col_name
        );
        let exists: i64 = conn.query_row(&check_sql, [], |row| row.get(0))?;

        if exists == 0 {
            let alter_sql = format!(
                "ALTER TABLE dlsite_metadata_cache ADD COLUMN {} {}",
                col_name, col_def
            );
            conn.execute(&alter_sql, [])?;
        }
        Ok(())
    };

    // Migrate old schema by adding missing columns
    add_column_if_missing("description", "TEXT")?;
    add_column_if_missing("work_type", "TEXT")?;
    add_column_if_missing("file_format", "TEXT")?;
    add_column_if_missing("tags_json", "TEXT")?;

    // Rename metadata_json to raw_api_json if needed
    let check_raw_api_json: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('dlsite_metadata_cache') WHERE name='raw_api_json'",
        [],
        |row| row.get(0),
    )?;

    if check_raw_api_json == 0 {
        // Check if old metadata_json exists
        let check_metadata: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('dlsite_metadata_cache') WHERE name='metadata_json'",
            [],
            |row| row.get(0)
        )?;

        if check_metadata > 0 {
            // Rename column
            conn.execute(
                "ALTER TABLE dlsite_metadata_cache RENAME COLUMN metadata_json TO raw_api_json",
                [],
            )?;
        } else {
            // Neither exists, add raw_api_json
            add_column_if_missing("raw_api_json", "TEXT NOT NULL DEFAULT '{}'")?;
        }
    }

    // Create index on cached_at for efficient cleanup
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_cached_at ON dlsite_metadata_cache(cached_at)",
        [],
    )
    .context("Failed to create index on cached_at")?;

    Ok(())
}
