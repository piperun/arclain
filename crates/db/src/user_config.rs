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

    /// Enabled plugins (JSON list of IDs)
    #[db(nullable)]
    pub enabled_plugins: Option<String>,

    /// Plugin order (JSON list of IDs)
    #[db(nullable)]
    pub plugin_order: Option<String>,

    /// Plugin visibility settings (JSON map)
    #[db(nullable)]
    pub plugin_visibility: Option<String>,

    /// Plugin specific settings (JSON map of PluginID -> Map<Key, Value>)
    #[db(nullable)]
    pub plugin_settings: Option<String>,
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

    // --- Plugin Headers ---

    pub fn get_enabled_plugins(&self) -> Vec<String> {
        self.enabled_plugins
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    pub fn set_enabled_plugins(&mut self, plugins: &[String]) {
        self.enabled_plugins = serde_json::to_string(plugins).ok();
    }

    pub fn get_plugin_order(&self) -> Vec<String> {
        self.plugin_order
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    pub fn set_plugin_order(&mut self, order: &[String]) {
        self.plugin_order = serde_json::to_string(order).ok();
    }

    pub fn get_plugin_settings(
        &self,
        plugin_id: &str,
    ) -> std::collections::HashMap<String, String> {
        if let Some(json) = &self.plugin_settings {
            if let Ok(all_settings) = serde_json::from_str::<
                std::collections::HashMap<String, std::collections::HashMap<String, String>>,
            >(json)
            {
                return all_settings.get(plugin_id).cloned().unwrap_or_default();
            }
        }
        std::collections::HashMap::new()
    }

    pub fn get_all_plugin_settings(
        &self,
    ) -> std::collections::HashMap<String, std::collections::HashMap<String, String>> {
        self.plugin_settings
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    pub fn set_plugin_settings(
        &mut self,
        plugin_id: &str,
        settings: std::collections::HashMap<String, String>,
    ) {
        let mut all_settings = self.get_all_plugin_settings();
        all_settings.insert(plugin_id.to_string(), settings);
        self.plugin_settings = serde_json::to_string(&all_settings).ok();
    }

    pub fn set_all_plugin_settings(
        &mut self,
        all_settings: &std::collections::HashMap<String, std::collections::HashMap<String, String>>,
    ) {
        self.plugin_settings = serde_json::to_string(all_settings).ok();
    }
}
