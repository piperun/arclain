use crate::diesel_err;
use anyhow::Result;
use crate::SqliteDb;
use rusqlite::Connection;
use std::path::Path;

/// Chmod `path` to `0o600` (owner read+write only) on Unix. No-op on
/// Windows since NTFS ACLs default to inheriting the user's profile
/// permissions, which already restricts cross-user access for files
/// in `%LOCALAPPDATA%`.
#[allow(unused_variables)]
fn restrict_to_owner(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use anyhow::Context;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", path.display()))?;
    }
    Ok(())
}

/// Configuration database handling user preferences and organization rules
pub struct ConfigDb {
    db: SqliteDb,
}

impl ConfigDb {
    /// Open the configuration database, creating tables if they don't exist.
    ///
    /// On Unix, the DB file is chmod'd to `0o600` after creation so it can't
    /// be read by other local users — SQLite respects the process umask
    /// when creating the file, which on most Linux distros is `0o022`
    /// (world-readable). The config DB carries plugin proxy settings,
    /// path overrides, and other user-private state that shouldn't leak
    /// across accounts on a shared system.
    pub fn open(path: &Path) -> Result<Self> {
        let db = SqliteDb::open(path)?;
        db.init_schema(Self::init_schema)?;
        restrict_to_owner(path)?;
        Ok(Self { db })
    }

    /// Open an in-memory configuration database (for testing)
    pub fn open_in_memory() -> Result<Self> {
        let db = SqliteDb::open_in_memory()?;
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
                temp_dir TEXT,
                sevenzip_path TEXT,
                transfer_dir TEXT,
                backend_mode TEXT NOT NULL DEFAULT 'native',
                open_nested_in_new_tab INTEGER NOT NULL DEFAULT 0,
                enabled_plugins TEXT,
                plugin_order TEXT,
                plugin_visibility TEXT,
                plugin_settings TEXT,
                toolbar_order TEXT,
                info_panel_order TEXT,
                socks5_address TEXT,
                socks5_enabled INTEGER NOT NULL DEFAULT 0,
                socks5_username TEXT,
                plugin_proxy_settings TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                modified_at TEXT
            );",
            [],
        )?;

        // Migrate existing user_config tables - add missing columns
        Self::migrate_user_config(conn)?;

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

        conn.execute(
            "CREATE TABLE IF NOT EXISTS archive_profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                description TEXT,
                format TEXT NOT NULL DEFAULT '7z',
                compression_level INTEGER NOT NULL DEFAULT 9,
                compression_method TEXT,
                solid_archive INTEGER NOT NULL DEFAULT 1,
                encrypt_headers INTEGER NOT NULL DEFAULT 0,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_system INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                modified_at TEXT
            );",
            [],
        )?;

        // Seed default archive profiles if table is empty
        Self::seed_default_archive_profiles(conn)?;

        // Initialize UI configuration tables
        crate::ui::ensure_ui_tables(conn)?;
        crate::ui::seed_defaults_if_empty(conn)?;

        // Initialize domain whitelist table
        crate::domain_whitelist::ensure_whitelist_table(conn)?;

        // Pipeline execution history table
        crate::pipeline_runs::ensure_pipeline_runs_table(conn)?;

        Ok(())
    }

    /// Migrate user_config table to add missing columns
    fn migrate_user_config(conn: &Connection) -> Result<()> {
        // Helper to add column if missing
        let add_column_if_missing = |col_name: &str, col_def: &str| -> Result<()> {
            let check_sql = format!(
                "SELECT COUNT(*) FROM pragma_table_info('user_config') WHERE name='{}'",
                col_name
            );
            let exists: i64 = conn.query_row(&check_sql, [], |row| row.get(0))?;

            if exists == 0 {
                let alter_sql = format!(
                    "ALTER TABLE user_config ADD COLUMN {} {}",
                    col_name, col_def
                );
                tracing::info!(
                    "[ConfigDb] Migrating: adding column {} to user_config",
                    col_name
                );
                conn.execute(&alter_sql, [])?;
            }
            Ok(())
        };

        // Add all columns that might be missing from older schemas
        add_column_if_missing("temp_dir", "TEXT")?;
        add_column_if_missing("sevenzip_path", "TEXT")?;
        add_column_if_missing("transfer_dir", "TEXT")?;
        add_column_if_missing("backend_mode", "TEXT NOT NULL DEFAULT 'native'")?;
        add_column_if_missing("open_nested_in_new_tab", "INTEGER NOT NULL DEFAULT 0")?;
        add_column_if_missing("enabled_plugins", "TEXT")?;
        add_column_if_missing("plugin_order", "TEXT")?;
        add_column_if_missing("plugin_visibility", "TEXT")?;
        add_column_if_missing("plugin_settings", "TEXT")?;
        add_column_if_missing("toolbar_order", "TEXT")?;
        add_column_if_missing("info_panel_order", "TEXT")?;
        add_column_if_missing("socks5_address", "TEXT")?;
        add_column_if_missing("socks5_enabled", "INTEGER NOT NULL DEFAULT 0")?;
        add_column_if_missing("socks5_username", "TEXT")?;
        add_column_if_missing("plugin_proxy_settings", "TEXT")?;

        Ok(())
    }

    /// Seed default archive profiles if the table is empty
    fn seed_default_archive_profiles(conn: &Connection) -> Result<()> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM archive_profiles",
            [],
            |row| row.get(0),
        )?;

        if count == 0 {
            tracing::info!("[ConfigDb] Seeding default archive profiles");

            // Maximum Compression (7z) - default
            conn.execute(
                "INSERT INTO archive_profiles
                 (name, description, format, compression_level, compression_method, solid_archive, is_default, is_system)
                 VALUES ('Maximum Compression (7z)', 'Best compression ratio, slower speed. Uses LZMA2 algorithm.', '7z', 9, 'LZMA2', 1, 1, 1)",
                [],
            )?;

            // Fast Compression (7z)
            conn.execute(
                "INSERT INTO archive_profiles
                 (name, description, format, compression_level, compression_method, solid_archive, is_system)
                 VALUES ('Fast Compression (7z)', 'Quick compression for large archives. Lower ratio but much faster.', '7z', 1, 'LZMA2', 0, 1)",
                [],
            )?;

            // Zip Compatible
            conn.execute(
                "INSERT INTO archive_profiles
                 (name, description, format, compression_level, compression_method, solid_archive, is_system)
                 VALUES ('Zip Compatible', 'Standard ZIP format for maximum compatibility with other tools.', 'zip', 9, 'Deflate', 0, 1)",
                [],
            )?;
        }

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

/// Title Replacement Model - Diesel ORM compatible
#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::diesel_schema::title_replacements)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct DbTitleReplacement {
    pub id: i32,
    pub original: String,
    pub replacement: String,
    pub is_system: bool,
}

/// For inserting new title replacements
#[derive(diesel::Insertable)]
#[diesel(table_name = crate::diesel_schema::title_replacements)]
pub struct NewTitleReplacement<'a> {
    pub original: &'a str,
    pub replacement: &'a str,
    pub is_system: bool,
}

/// Title Filter Settings Model
#[derive(Debug, Clone)]
pub struct DbTitleFilterSettings {
    pub invalid_chars: Option<String>,
    pub replacement: Option<String>,
    pub max_length: Option<usize>,
    pub trim_whitespace: Option<bool>,
}

// ============================================================================
// Diesel DSL CRUD
// ============================================================================

use diesel::prelude::*;

/// List all title replacements
pub fn list_title_replacements(
    conn: &mut diesel::SqliteConnection,
) -> Result<Vec<DbTitleReplacement>> {
    use crate::diesel_schema::title_replacements::dsl::*;

    let results = title_replacements
        .select((id, original, replacement, is_system))
        .order(original.asc())
        .load::<(i32, String, String, bool)>(conn)
        .map_err(diesel_err("query"))?;

    Ok(results
        .into_iter()
        .map(|(i, o, r, s)| DbTitleReplacement {
            id: i,
            original: o,
            replacement: r,
            is_system: s,
        })
        .collect())
}

/// Save a title replacement (upsert by original)
pub fn save_title_replacement(
    conn: &mut diesel::SqliteConnection,
    orig: &str,
    repl: &str,
    sys: bool,
) -> Result<()> {
    use crate::diesel_schema::title_replacements::dsl::*;

    diesel::insert_into(title_replacements)
        .values((original.eq(orig), replacement.eq(repl), is_system.eq(sys)))
        .on_conflict(original)
        .do_update()
        .set(replacement.eq(repl))
        .execute(conn)
        .map_err(diesel_err("insert"))?;

    Ok(())
}

/// Delete a title replacement (system replacements are immune)
pub fn delete_title_replacement(
    conn: &mut diesel::SqliteConnection,
    rule_id: i32,
) -> Result<()> {
    use crate::diesel_schema::title_replacements::dsl::*;

    diesel::delete(title_replacements.filter(id.eq(rule_id).and(is_system.eq(false))))
        .execute(conn)
        .map_err(diesel_err("delete"))?;

    Ok(())
}

/// Read title-filter scalar settings (invalid_chars, replacement, etc.)
/// from the `app_config` key-value table.
pub fn get_title_filter_settings(
    conn: &mut diesel::SqliteConnection,
) -> Result<DbTitleFilterSettings> {
    let invalid_chars = crate::get_config_diesel(conn, "title_filter.invalid_chars")?;
    let replacement = crate::get_config_diesel(conn, "title_filter.replacement")?;
    let max_length = crate::get_config_diesel(conn, "title_filter.max_length")?
        .and_then(|s| s.parse().ok());
    let trim_whitespace = crate::get_config_diesel(conn, "title_filter.trim_whitespace")?
        .and_then(|s| s.parse().ok());

    Ok(DbTitleFilterSettings {
        invalid_chars,
        replacement,
        max_length,
        trim_whitespace,
    })
}

/// Persist title-filter scalar settings to the `app_config` table.
pub fn save_title_filter_settings(
    conn: &mut diesel::SqliteConnection,
    settings: &DbTitleFilterSettings,
) -> Result<()> {
    if let Some(val) = &settings.invalid_chars {
        crate::set_config_diesel(conn, "title_filter.invalid_chars", val)?;
    }
    if let Some(val) = &settings.replacement {
        crate::set_config_diesel(conn, "title_filter.replacement", val)?;
    }
    if let Some(val) = settings.max_length {
        crate::set_config_diesel(conn, "title_filter.max_length", &val.to_string())?;
    }
    if let Some(val) = settings.trim_whitespace {
        crate::set_config_diesel(conn, "title_filter.trim_whitespace", &val.to_string())?;
    }
    Ok(())
}
