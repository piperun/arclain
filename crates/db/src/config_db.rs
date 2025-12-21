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

        conn.execute(
            "CREATE TABLE IF NOT EXISTS title_replacements (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                original TEXT NOT NULL UNIQUE,
                replacement TEXT NOT NULL,
                is_system BOOLEAN NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
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

        // Initialize UI configuration tables
        crate::ui_config::ensure_ui_tables(conn)?;
        crate::ui_config::seed_defaults_if_empty(conn)?;

        // Initialize domain whitelist table
        crate::domain_whitelist::ensure_whitelist_table(conn)?;

        Ok(())
    }

    /// Convert into the underlying SqliteDb wrapper
    pub fn into_sqlite_db(self) -> SqliteDb {
        self.db
    }

    /// Execute a closure with access to the underlying connection
    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        self.db.with_connection(f)
    }
}

/// Title Replacement Model
#[derive(Debug, Clone)]
pub struct DbTitleReplacement {
    pub id: i64,
    pub original: String,
    pub replacement: String,
    pub is_system: bool,
}

// DB Operations for Title Replacements
pub fn list_title_replacements(conn: &Connection) -> Result<Vec<DbTitleReplacement>> {
    let mut stmt = conn.prepare(
        "SELECT id, original, replacement, is_system FROM title_replacements ORDER BY original",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DbTitleReplacement {
            id: row.get(0)?,
            original: row.get(1)?,
            replacement: row.get(2)?,
            is_system: row.get(3)?,
        })
    })?;

    let mut replacements = Vec::new();
    for row in rows {
        replacements.push(row?);
    }
    Ok(replacements)
}

pub fn save_title_replacement(
    conn: &Connection,
    original: &str,
    replacement: &str,
    is_system: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO title_replacements (original, replacement, is_system) VALUES (?1, ?2, ?3)
         ON CONFLICT(original) DO UPDATE SET replacement = excluded.replacement",
        (original, replacement, is_system),
    )?;
    Ok(())
}

/// Title Filter Settings Model
#[derive(Debug, Clone)]
pub struct DbTitleFilterSettings {
    pub invalid_chars: Option<String>,
    pub replacement: Option<String>,
    pub max_length: Option<usize>,
    pub trim_whitespace: Option<bool>,
}

// DB Operations for Title Filter Settings
pub fn get_title_filter_settings(conn: &Connection) -> Result<DbTitleFilterSettings> {
    let invalid_chars = crate::get_config(conn, "title_filter.invalid_chars")?;
    let replacement = crate::get_config(conn, "title_filter.replacement")?;
    let max_length =
        crate::get_config(conn, "title_filter.max_length")?.and_then(|s| s.parse().ok());
    let trim_whitespace =
        crate::get_config(conn, "title_filter.trim_whitespace")?.and_then(|s| s.parse().ok());

    Ok(DbTitleFilterSettings {
        invalid_chars,
        replacement,
        max_length,
        trim_whitespace,
    })
}

pub fn save_title_filter_settings(
    conn: &Connection,
    settings: &DbTitleFilterSettings,
) -> Result<()> {
    if let Some(val) = &settings.invalid_chars {
        crate::set_config(conn, "title_filter.invalid_chars", val)?;
    }
    if let Some(val) = &settings.replacement {
        crate::set_config(conn, "title_filter.replacement", val)?;
    }
    if let Some(val) = settings.max_length {
        crate::set_config(conn, "title_filter.max_length", &val.to_string())?;
    }
    if let Some(val) = settings.trim_whitespace {
        crate::set_config(conn, "title_filter.trim_whitespace", &val.to_string())?;
    }
    Ok(())
}

pub fn delete_title_replacement(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM title_replacements WHERE id = ?1 AND is_system = 0",
        [id],
    )?;
    Ok(())
}
