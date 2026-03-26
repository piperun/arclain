//! User configuration stored in the database.
//!
//! This struct uses the `DbConfig` derive macro to automatically generate
//! SQL table creation and CRUD methods.

use anyhow::Result;
use diesel::prelude::*;
use mini_orm::DbConfig;
use std::path::PathBuf;

/// User configuration settings stored in the database.
///
/// This replaces the old config.json file-based storage.
#[derive(Debug, Clone, Default, DbConfig, QueryableByName)]
#[db_table = "user_config"]
#[diesel(table_name = crate::diesel_schema::user_config)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
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

    /// Toolbar button order (JSON list of button IDs)
    #[db(nullable)]
    pub toolbar_order: Option<String>,

    /// Info panel section order (JSON list of section IDs)
    #[db(nullable)]
    pub info_panel_order: Option<String>,

    /// SOCKS5 proxy address (e.g., "127.0.0.1:1080")
    #[db(nullable)]
    pub socks5_address: Option<String>,

    /// Whether SOCKS5 proxy is enabled
    #[db(default = "0")]
    pub socks5_enabled: bool,

    /// Optional SOCKS5 username
    #[db(nullable)]
    pub socks5_username: Option<String>,

    /// Plugin proxy settings (JSON map of PluginID -> bool)
    #[db(nullable)]
    pub plugin_proxy_settings: Option<String>,

    /// Hotkey bindings (JSON map of ActionId -> HotkeyBinding)
    #[db(nullable)]
    pub hotkey_bindings: Option<String>,

    /// Whether gameta server integration is enabled
    #[db(default = "0")]
    pub gameta_server_enabled: bool,

    /// Gameta server URL (e.g., "https://gameta.example.com")
    #[db(nullable)]
    pub gameta_server_url: Option<String>,
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

    // --- Toolbar Order ---

    pub fn get_toolbar_order(&self) -> Vec<String> {
        self.toolbar_order
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    pub fn set_toolbar_order(&mut self, order: &[String]) {
        self.toolbar_order = serde_json::to_string(order).ok();
    }

    // --- Info Panel Order ---

    pub fn get_info_panel_order(&self) -> Vec<String> {
        self.info_panel_order
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    pub fn set_info_panel_order(&mut self, order: &[String]) {
        self.info_panel_order = serde_json::to_string(order).ok();
    }

    // --- Plugin Proxy Settings ---

    pub fn get_plugin_proxy_settings(&self) -> std::collections::HashMap<String, bool> {
        self.plugin_proxy_settings
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    pub fn set_plugin_proxy_settings(
        &mut self,
        settings: &std::collections::HashMap<String, bool>,
    ) {
        self.plugin_proxy_settings = serde_json::to_string(settings).ok();
    }

    pub fn set_plugin_proxy_enabled(&mut self, plugin_id: &str, enabled: bool) {
        let mut settings = self.get_plugin_proxy_settings();
        settings.insert(plugin_id.to_string(), enabled);
        self.set_plugin_proxy_settings(&settings);
    }

    // --- Hotkey Bindings ---

    /// Get hotkey bindings as a map of action_id -> binding JSON
    pub fn get_hotkey_bindings(&self) -> std::collections::HashMap<String, String> {
        self.hotkey_bindings
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    /// Set hotkey bindings from a map of action_id -> binding JSON
    pub fn set_hotkey_bindings(&mut self, bindings: &std::collections::HashMap<String, String>) {
        self.hotkey_bindings = serde_json::to_string(bindings).ok();
    }

    /// Get binding for a specific action
    pub fn get_hotkey_binding(&self, action_id: &str) -> Option<String> {
        self.get_hotkey_bindings().get(action_id).cloned()
    }

    /// Set binding for a specific action
    pub fn set_hotkey_binding(&mut self, action_id: &str, binding: &str) {
        let mut bindings = self.get_hotkey_bindings();
        bindings.insert(action_id.to_string(), binding.to_string());
        self.set_hotkey_bindings(&bindings);
    }

    /// Remove binding for a specific action
    pub fn remove_hotkey_binding(&mut self, action_id: &str) {
        let mut bindings = self.get_hotkey_bindings();
        bindings.remove(action_id);
        self.set_hotkey_bindings(&bindings);
    }

    /// Load config from Diesel connection (singleton row 1)
    pub fn load_diesel(conn: &mut diesel::SqliteConnection) -> Result<Self> {
        use crate::diesel_schema::user_config::dsl::*;

        // Use QueryableByName to explicitly map columns if needed, or Selectable.
        // Given the trouble with auto-derives, let's use a simpler approach.
        // We select the tuple and map it manually.

        let result = user_config
            .find(1)
            .select((
                id,
                vault_path,
                cache_directory,
                last_opened_archive,
                temp_dir,
                sevenzip_path,
                transfer_dir,
                backend_mode,
                open_nested_in_new_tab,
                enabled_plugins,
                plugin_order,
                plugin_visibility,
                plugin_settings,
                toolbar_order,
                info_panel_order,
                socks5_address,
                socks5_enabled,
                socks5_username,
                plugin_proxy_settings,
                hotkey_bindings,
                gameta_server_enabled,
                gameta_server_url,
            ))
            .first::<(
                i32,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                String,
                bool,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                bool,
                Option<String>,
                Option<String>,
                Option<String>,
                bool,
                Option<String>,
            )>(conn);

        match result {
            Ok(tuple) => Ok(UserConfig {
                id: tuple.0,
                vault_path: tuple.1,
                cache_directory: tuple.2,
                last_opened_archive: tuple.3,
                temp_dir: tuple.4,
                sevenzip_path: tuple.5,
                transfer_dir: tuple.6,
                backend_mode: tuple.7,
                open_nested_in_new_tab: tuple.8,
                enabled_plugins: tuple.9,
                plugin_order: tuple.10,
                plugin_visibility: tuple.11,
                plugin_settings: tuple.12,
                toolbar_order: tuple.13,
                info_panel_order: tuple.14,
                socks5_address: tuple.15,
                socks5_enabled: tuple.16,
                socks5_username: tuple.17,
                plugin_proxy_settings: tuple.18,
                hotkey_bindings: tuple.19,
                gameta_server_enabled: tuple.20,
                gameta_server_url: tuple.21,
            }),
            Err(diesel::result::Error::NotFound) => {
                // If not found, create default and insert it manually (providing all non-nullable fields)
                diesel::insert_into(user_config)
                    .values((
                        id.eq(1),
                        backend_mode.eq("native"),
                        open_nested_in_new_tab.eq(false),
                        socks5_enabled.eq(false),
                        gameta_server_enabled.eq(false),
                        created_at.eq(diesel::dsl::sql::<diesel::sql_types::Text>(
                            "CURRENT_TIMESTAMP",
                        )),
                    ))
                    .execute(conn)?;
                Ok(UserConfig::new())
            }
            Err(e) => Err(anyhow::anyhow!("Failed to load user config: {}", e)),
        }
    }

    /// Save config to Diesel connection (uses UPSERT to handle missing row)
    pub fn save_diesel(&self, conn: &mut diesel::SqliteConnection) -> Result<()> {
        let sql = r#"
            INSERT INTO user_config (
                id, vault_path, cache_directory, last_opened_archive, temp_dir,
                sevenzip_path, transfer_dir, backend_mode, open_nested_in_new_tab,
                enabled_plugins, plugin_order, plugin_visibility, plugin_settings,
                toolbar_order, info_panel_order, socks5_address, socks5_enabled,
                socks5_username, plugin_proxy_settings, hotkey_bindings,
                gameta_server_enabled, gameta_server_url, created_at, modified_at
            ) VALUES (
                1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
                ?20, ?21, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
            )
            ON CONFLICT(id) DO UPDATE SET
                vault_path = excluded.vault_path,
                cache_directory = excluded.cache_directory,
                last_opened_archive = excluded.last_opened_archive,
                temp_dir = excluded.temp_dir,
                sevenzip_path = excluded.sevenzip_path,
                transfer_dir = excluded.transfer_dir,
                backend_mode = excluded.backend_mode,
                open_nested_in_new_tab = excluded.open_nested_in_new_tab,
                enabled_plugins = excluded.enabled_plugins,
                plugin_order = excluded.plugin_order,
                plugin_visibility = excluded.plugin_visibility,
                plugin_settings = excluded.plugin_settings,
                toolbar_order = excluded.toolbar_order,
                info_panel_order = excluded.info_panel_order,
                socks5_address = excluded.socks5_address,
                socks5_enabled = excluded.socks5_enabled,
                socks5_username = excluded.socks5_username,
                plugin_proxy_settings = excluded.plugin_proxy_settings,
                hotkey_bindings = excluded.hotkey_bindings,
                gameta_server_enabled = excluded.gameta_server_enabled,
                gameta_server_url = excluded.gameta_server_url,
                modified_at = CURRENT_TIMESTAMP
        "#;

        diesel::sql_query(sql)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&self.vault_path)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&self.cache_directory)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(
                &self.last_opened_archive,
            )
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&self.temp_dir)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&self.sevenzip_path)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&self.transfer_dir)
            .bind::<diesel::sql_types::Text, _>(&self.backend_mode)
            .bind::<diesel::sql_types::Bool, _>(self.open_nested_in_new_tab)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&self.enabled_plugins)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&self.plugin_order)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(
                &self.plugin_visibility,
            )
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&self.plugin_settings)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&self.toolbar_order)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&self.info_panel_order)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&self.socks5_address)
            .bind::<diesel::sql_types::Bool, _>(self.socks5_enabled)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&self.socks5_username)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(
                &self.plugin_proxy_settings,
            )
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&self.hotkey_bindings)
            .bind::<diesel::sql_types::Bool, _>(self.gameta_server_enabled)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(
                &self.gameta_server_url,
            )
            .execute(conn)
            .map_err(|e| {
                tracing::error!("[UserConfig] save_diesel UPSERT failed: {}", e);
                anyhow::anyhow!("Failed to save user config: {}", e)
            })?;

        tracing::info!("[UserConfig] save_diesel: config saved successfully");
        Ok(())
    }
}
