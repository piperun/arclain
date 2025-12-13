//! User configuration stored in the database.
//!
//! This struct uses the `DbConfig` derive macro to automatically generate
//! SQL table creation and CRUD methods.

use arclain_db_derive::DbConfig;
use std::path::PathBuf;

/// User configuration settings stored in the database.
///
/// This replaces the old config.json file-based storage.
#[derive(Debug, Clone, Default, DbConfig)]
#[db_table = "user_config"]
pub struct UserConfig {
    /// Primary key (always 1 for single-row pattern)
    #[db(primary_key)]
    pub id: i32,

    /// Path to secrets vault database
    #[db(nullable)]
    pub vault_path: Option<String>,

    /// Cache directory path
    #[db(nullable)]
    pub cache_directory: Option<String>,

    /// Last opened archive path
    #[db(nullable)]
    pub last_opened_archive: Option<String>,

    /// Temporary directory for extraction operations
    #[db(nullable)]
    pub temp_dir: Option<String>,

    /// Path to 7-Zip executable
    #[db(nullable)]
    pub sevenzip_path: Option<String>,

    /// Transfer directory for file operations
    #[db(nullable)]
    pub transfer_dir: Option<String>,

    /// Backend mode: "native" or "cli"
    #[db(default = "native")]
    pub backend_mode: String,

    /// Whether to open nested archives in a new tab
    #[db(default = "0")]
    pub open_nested_in_new_tab: bool,
}

impl UserConfig {
    /// Create a new UserConfig with default values
    pub fn new() -> Self {
        Self {
            id: 1,
            backend_mode: "native".to_string(),
            ..Default::default()
        }
    }

    /// Get temp_dir as PathBuf
    pub fn temp_dir_path(&self) -> Option<PathBuf> {
        self.temp_dir.as_ref().map(PathBuf::from)
    }

    /// Get sevenzip_path as PathBuf
    pub fn sevenzip_path_path(&self) -> Option<PathBuf> {
        self.sevenzip_path.as_ref().map(PathBuf::from)
    }

    /// Get transfer_dir as PathBuf
    pub fn transfer_dir_path(&self) -> Option<PathBuf> {
        self.transfer_dir.as_ref().map(PathBuf::from)
    }
}
