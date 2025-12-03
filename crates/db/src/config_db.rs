use crate::sqlite_db::SqliteDb;
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

/// Configuration database handling user preferences and organization rules
pub struct ConfigDb {
    db: SqliteDb,
}

impl ConfigDb {
    /// Open the configuration database, creating tables if they don't exist
    pub fn open(path: &Path) -> Result<Self> {
        let db = SqliteDb::open(path)?;
        db.init_schema(Self::init_schema)?;
        Ok(Self { db })
    }

    /// Initialize the database schema
    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_config (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                vault_path TEXT,
                cache_directory TEXT,
                last_opened_archive TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                modified_at TEXT
            );",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS app_config(
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS organization_rules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                description TEXT,
                category TEXT NOT NULL,
                priority INTEGER NOT NULL DEFAULT 0,
                is_enabled BOOLEAN NOT NULL DEFAULT 1,
                is_system BOOLEAN NOT NULL DEFAULT 0,
                trigger_json TEXT NOT NULL,
                actions_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                modified_at TEXT
            );",
            [],
        )?;

        // Ensure a meta row exists for migrations (if we want to use them later)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS meta(
                migration INTEGER NOT NULL
            );",
            [],
        )?;

        conn.execute(
            "INSERT INTO meta (migration) SELECT 0 WHERE NOT EXISTS (SELECT 1 FROM meta);",
            [],
        )?;

        Ok(())
    }

    /// Convert into the underlying SqliteDb wrapper
    pub fn into_sqlite_db(self) -> SqliteDb {
        self.db
    }
}
