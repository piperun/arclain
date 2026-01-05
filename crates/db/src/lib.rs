use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rusqlite::{Connection, OptionalExtension};
use std::fs;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

// SqliteDb comes from mini-orm (shared ORM crate)
pub use mini_orm::SqliteDb;

mod redb_wrapper;
pub use redb_wrapper::ReDb;

mod config_db;
pub use config_db::{
    delete_title_replacement, get_title_filter_settings, list_title_replacements,
    save_title_filter_settings, save_title_replacement, ConfigDb, DbTitleFilterSettings,
    DbTitleReplacement,
};

mod cache_db;
pub use cache_db::CacheDb;

mod secrets;
pub use secrets::{PassRule as DbPassRule, SecretsDb};

mod metadata_store;
pub use metadata_store::MetadataStore;

mod organization;
pub use organization::{delete_rule, get_rule, list_rules, save_rule, DbOrganizationRule};

mod checksum_db;

pub use checksum_db::{
    begin_checksum_operation, delete_checksum_operation, get_checksum_algorithm, get_checksum_mode,
    get_file_checksum, get_merkle_root, get_pending_checksum_operations, set_checksum_algorithm,
    set_checksum_mode, store_file_checksum, store_merkle_root, update_checksum_operation,
    ChecksumDb, DbFileChecksum, DbOperation, OpId, OpState, OpType, VerifyMode,
};
pub use mini_orm::{JoinType, OrderDirection, QueryBuilder};

mod cache_index;
pub use cache_index::{
    clear_all_entries, delete_cache_entry, get_cache_entry, get_entries_by_product,
    has_cache_entry, init_cache_index_schema, touch_cache_entry, upsert_cache_entry, CacheEntry,
    CacheType,
};

/// Re-export derive macro from mini-orm
pub use mini_orm::DbConfig;

mod user_config;
pub use user_config::UserConfig;

mod ui_config;
pub use ui_config::{
    delete_item, ensure_ui_tables, get_display_option, get_item, get_region_config,
    list_items_by_region, seed_defaults_if_empty, set_display_option, set_item_display_mode,
    set_item_order, set_item_visibility, upsert_item, upsert_region_config, ActionType,
    DisplayMode, UiItem, UiRegion, UiRegionConfig,
};

mod domain_whitelist;
pub use domain_whitelist::{
    approve_domain, delete_plugin_whitelist, delete_whitelist_entry, domain_exists,
    ensure_whitelist_table, is_domain_approved, list_pending_approvals, list_plugin_domains,
    list_whitelist_entries, revoke_domain, upsert_whitelist_entry, DbWhitelistEntry,
};

mod product_metadata;
pub use product_metadata::{
    delete as delete_product_metadata, get_by_external_id, init_product_metadata_schema,
    list_by_source, load as load_product_metadata, save as save_product_metadata, MetadataSource,
    ProductMetadata,
};

mod product_content;
pub use product_content::{
    delete_product_content, get_all_content, get_cover, get_screenshots,
    init_product_content_schema, save as save_product_content, ContentType, ProductContent,
};

/// Re-export Connection so dependents don't need rusqlite directly.
pub use rusqlite::Connection as DbConnection;

/// Canonical paths for the two databases and optional key-file
#[derive(Debug, Clone)]
pub struct DbPaths {
    pub config_db: PathBuf,
    pub cache_db: PathBuf,
    pub secrets_db: PathBuf,
    pub key_file: Option<PathBuf>,
}

impl DbPaths {
    /// Defaults:
    /// - config.sqlite at %APPDATA%/{app}/
    /// - pass.redb at %APPDATA%/{app}/secrets/ (redb with AES-256-GCM)
    /// - master key at %APPDATA%/{app}/secrets/master.key
    pub fn defaults(app_name: &str) -> Result<Self> {
        let base = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(app_name);
        let secrets_dir = base.join("secrets");

        // Create directories with secure permissions
        fs::create_dir_all(&base)
            .with_context(|| format!("creating config dir {}", base.display()))?;
        fs::create_dir_all(&secrets_dir)
            .with_context(|| format!("creating secrets dir {}", secrets_dir.display()))?;

        // Set directory permissions to 700 (user-only) on Unix
        #[cfg(unix)]
        {
            let perms = fs::Permissions::from_mode(0o700);
            fs::set_permissions(&secrets_dir, perms)
                .with_context(|| format!("setting permissions on {}", secrets_dir.display()))?;
        }

        Ok(Self {
            config_db: base.join("config.sqlite"),
            cache_db: base.join("metadata.sqlite"),
            secrets_db: secrets_dir.join("pass.redb"),
            key_file: Some(secrets_dir.join("master.key")),
        })
    }
}

/// Zeroizing in-memory holder for the 32-byte AES encryption key
pub struct SecretsKey(pub Zeroizing<Vec<u8>>);

// Custom Debug implementation to avoid logging key material
impl std::fmt::Debug for SecretsKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretsKey")
            .field("len", &self.0.len())
            .field("data", &"[REDACTED]")
            .finish()
    }
}

impl SecretsKey {
    /// Generate a new random 32-byte key
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut key = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        Self(Zeroizing::new(key))
    }

    /// Save the key to a file in base64 format with secure permissions
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        // Validate path to prevent directory traversal
        validate_path(path)?;

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;

            // Set directory permissions to 700 (user-only) on Unix
            #[cfg(unix)]
            {
                let perms = fs::Permissions::from_mode(0o700);
                fs::set_permissions(parent, perms)
                    .with_context(|| format!("setting permissions on {}", parent.display()))?;
            }
        }

        let encoded = B64.encode(&*self.0);
        fs::write(path, encoded).with_context(|| format!("writing key to {}", path.display()))?;

        // Set file permissions to 600 (read/write user only) on Unix
        #[cfg(unix)]
        {
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(path, perms)
                .with_context(|| format!("setting permissions on {}", path.display()))?;
        }

        Ok(())
    }

    /// Load the key from a file
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("reading key from {}", path.display()))?;
        let bytes = B64
            .decode(contents.trim())
            .context("Invalid base64 in key file")?;

        if bytes.len() != 32 {
            return Err(anyhow!("Invalid key length: expected 32 bytes"));
        }

        Ok(Self(Zeroizing::new(bytes)))
    }

    /// Get the 32-byte key as a fixed-size array
    pub fn as_bytes(&self) -> [u8; 32] {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&self.0);
        arr
    }

    /// Return hex string (for debugging/logging)
    pub fn as_hex_upper(&self) -> String {
        hex_encode_upper(&self.0)
    }
}

/// Validate that a path is safe (no parent traversal)
fn validate_path(path: &Path) -> Result<()> {
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(anyhow!("Invalid path: contains parent directory traversal"));
        }
    }
    Ok(())
}

/// Holds open connections to both databases
pub struct ConfigDbs {
    pub config: SqliteDb,
    pub secrets: SecretsDb,
    pub metadata: MetadataStore,
}

/// Open all databases, initializing schemas if needed
pub fn open_databases(paths: &DbPaths, key: &SecretsKey) -> Result<ConfigDbs> {
    // Open config database using new module
    let config_db = ConfigDb::open(&paths.config_db)
        .with_context(|| format!("Failed to open config database at {:?}", paths.config_db))?;

    // Ensure cache directory exists
    if let Some(parent) = paths.cache_db.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create cache directory at {:?}", parent))?;
    }

    // Open cache database
    let cache_db = CacheDb::open(&paths.cache_db)
        .with_context(|| format!("Failed to open cache database at {:?}", paths.cache_db))?;

    // Open secrets database using new module
    let secrets_db = SecretsDb::open(&paths.secrets_db, &key.as_bytes())
        .with_context(|| format!("Failed to open secrets database at {:?}", paths.secrets_db))?;

    Ok(ConfigDbs {
        config: config_db.into_sqlite_db(),
        secrets: secrets_db,
        metadata: MetadataStore::new(
            cache_db.into_sqlite_db(),
            paths
                .cache_db
                .parent()
                .unwrap_or(Path::new("."))
                .join("metadata"),
        ),
    })
}

/// Simple K/V config helpers (stored in plain config.sqlite)
pub fn get_config(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM app_config WHERE key = ?1")?;
    let val = stmt
        .query_row([key], |row| row.get::<_, String>(0))
        .optional()?;
    Ok(val)
}

pub fn set_config(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO app_config(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (key, value),
    )?;
    Ok(())
}

/// Helpers

fn hex_encode_upper(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &v in bytes {
        s.push(HEX[(v >> 4) as usize] as char);
        s.push(HEX[(v & 0x0F) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests;
