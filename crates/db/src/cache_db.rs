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
    conn.execute(
        "CREATE TABLE IF NOT EXISTS dlsite_metadata_cache (
            product_id TEXT PRIMARY KEY,
            title TEXT,
            circle TEXT,
            price INTEGER,
            release_date TEXT,
            metadata_json TEXT NOT NULL,
            cached_at INTEGER NOT NULL
        )",
        [],
    )
    .context("Failed to create dlsite_metadata_cache table")?;

    // Create index on cached_at for efficient cleanup
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_cached_at ON dlsite_metadata_cache(cached_at)",
        [],
    )
    .context("Failed to create index on cached_at")?;

    Ok(())
}
