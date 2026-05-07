//! Database bootstrap: canonical paths and open-everything entry point.
//!
//! Extracted out of `lib.rs` (audit module-org callout) so the
//! `DbPaths` type and `open_databases` orchestration sit together
//! away from the crate-root re-export bookkeeping.

use crate::cache::CacheDb;
use crate::config::ConfigDb;
use crate::metadata_store::MetadataStore;
use crate::pool::DieselPool;
use crate::secrets::SecretsDb;
use crate::secrets_key::SecretsKey;
use anyhow::{Context, Result};
use crate::SqliteDb;
use std::path::{Path, PathBuf};

/// Canonical paths for the two databases and optional key-file
#[derive(Debug, Clone)]
pub struct DbPaths {
    pub config_db: PathBuf,
    pub cache_db: PathBuf,
    pub secrets_db: PathBuf,
    pub key_file: Option<PathBuf>,
}

impl DbPaths {
    /// Calculate default paths without creating them.
    /// Creation is now handled by arclain_core::dirs::AppDirectories.
    pub fn calculate_defaults(app_name: &str) -> Result<Self> {
        let base = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(app_name);

        let databases_dir = base.join("databases");
        let secrets_dir = base.join("secrets");

        Ok(Self {
            config_db: databases_dir.join("config.sqlite"),
            cache_db: databases_dir.join("metadata.sqlite"),
            secrets_db: secrets_dir.join("pass.redb"),
            key_file: Some(secrets_dir.join("master.key")),
        })
    }
}

/// Holds open connections to both databases
pub struct ConfigDbs {
    pub config: SqliteDb,
    pub secrets: SecretsDb,
    pub metadata: MetadataStore,
    pub config_pool: DieselPool,
    pub cache_pool: DieselPool,
}

/// Open all databases, initializing schemas if needed
pub fn open_databases(paths: &DbPaths, key: &SecretsKey) -> Result<ConfigDbs> {
    // Open config database using new module
    let config_db = ConfigDb::open(&paths.config_db)
        .with_context(|| format!("Failed to open config database at {:?}", paths.config_db))?;

    // Create Diesel pool for config
    let config_pool = DieselPool::new(&paths.config_db)
        .with_context(|| "Failed to create config database pool")?;

    // Open cache database
    let cache_db = CacheDb::open(&paths.cache_db)
        .with_context(|| format!("Failed to open cache database at {:?}", paths.cache_db))?;

    // Create Diesel pool for cache
    let cache_pool = DieselPool::new(&paths.cache_db)
        .with_context(|| "Failed to create cache database pool")?;

    // Open secrets database using new module
    let secrets_db = SecretsDb::open(&paths.secrets_db, &key.as_bytes())
        .with_context(|| format!("Failed to open secrets database at {:?}", paths.secrets_db))?;

    Ok(ConfigDbs {
        config: config_db.into_sqlite_db(),
        secrets: secrets_db,
        metadata: MetadataStore::new(
            cache_db.into_sqlite_db(),
            cache_pool.clone(),
            paths
                .cache_db
                .parent()
                .unwrap_or(Path::new("."))
                .join("metadata"),
            Some(paths.cache_db.clone()),
        ),
        config_pool,
        cache_pool,
    })
}
