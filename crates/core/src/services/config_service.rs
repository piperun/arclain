//! Configuration service for app config and user preferences
//!
//! Wraps arclain_db config functions with connection pool management.

use anyhow::Result;
use arclain_db::{get_config, set_config, DbConnection, DieselPool, UserConfig};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Service for managing application configuration
#[derive(Clone)]
pub struct ConfigService {
    pool: DieselPool,
    /// SQLite connection for rusqlite-based operations (get_config, set_config)
    sqlite_conn: Arc<Mutex<DbConnection>>,
}

impl ConfigService {
    /// Create a new config service
    pub fn new(pool: DieselPool, config_db_path: &Path) -> Result<Self> {
        let conn = DbConnection::open(config_db_path)?;
        Ok(Self {
            pool,
            sqlite_conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Create from existing connection (useful when you already have ConfigDbs)
    pub fn from_connection(pool: DieselPool, conn: DbConnection) -> Self {
        Self {
            pool,
            sqlite_conn: Arc::new(Mutex::new(conn)),
        }
    }

    // =========================================================================
    // Key-Value Config (rusqlite)
    // =========================================================================

    /// Get a config value by key
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        let conn = self.sqlite_conn.lock();
        get_config(&conn, key)
    }

    /// Set a config value
    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.sqlite_conn.lock();
        set_config(&conn, key, value)
    }

    /// Set a config value (Diesel version, uses pool)
    pub fn set_diesel(&self, key: &str, value: &str) -> Result<()> {
        self.pool
            .with_conn(|conn| arclain_db::set_config_diesel(conn, key, value))
    }

    // =========================================================================
    // User Config (Diesel)
    // =========================================================================

    /// Load user configuration
    pub fn get_user_config(&self) -> Result<UserConfig> {
        self.pool.with_conn(|conn| UserConfig::load_diesel(conn))
    }

    /// Save user configuration  
    pub fn save_user_config(&self, config: &UserConfig) -> Result<()> {
        self.pool.with_conn(|conn| config.save_diesel(conn))
    }

    // =========================================================================
    // Plugin Domain Management (Diesel)
    // =========================================================================

    /// Approve a domain for a plugin
    pub fn approve_plugin_domain(&self, plugin_id: &str, domain: &str) -> Result<()> {
        self.pool
            .with_conn(|conn| arclain_db::approve_domain(conn, plugin_id, domain))
    }

    /// Revoke a domain for a plugin
    pub fn revoke_plugin_domain(&self, plugin_id: &str, domain: &str) -> Result<()> {
        self.pool
            .with_conn(|conn| arclain_db::revoke_domain(conn, plugin_id, domain))
    }
    // =========================================================================
    // Startup Config Helpers
    // =========================================================================

    /// Load basic startup configuration without full app initialization
    pub fn load_startup_config(
        config_db_path: &Path,
    ) -> Result<(Option<PathBuf>, Option<PathBuf>, Option<String>)> {
        use arclain_db::ConfigDb;
        let db = ConfigDb::open(config_db_path)?;
        let conn = db.into_sqlite_db();

        conn.with_connection(|c| {
            let secrets_path = get_config(c, "secrets_db_path")?.map(PathBuf::from);
            let key_path = get_config(c, "key_file_path")?.map(PathBuf::from);
            let crc_policy = get_config(c, "encrypted_crc_policy")?;
            Ok((secrets_path, key_path, crc_policy))
        })
    }
}

impl std::fmt::Debug for ConfigService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigService")
            .field("pool", &self.pool)
            .finish()
    }
}
