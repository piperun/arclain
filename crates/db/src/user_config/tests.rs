//! Unit tests for user_config module

use super::*;
use diesel::prelude::*;

fn setup_diesel_conn() -> diesel::SqliteConnection {
    let mut conn =
        diesel::SqliteConnection::establish(":memory:").expect("Failed to create in-memory SQLite");

    // Create user_config table
    diesel::sql_query(
        r#"
        CREATE TABLE user_config (
            id INTEGER PRIMARY KEY,
            vault_path TEXT,
            cache_directory TEXT,
            last_opened_archive TEXT,
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
            hotkey_bindings TEXT,
            gameta_server_enabled INTEGER NOT NULL DEFAULT 0,
            gameta_server_url TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            modified_at TEXT
        )
        "#,
    )
    .execute(&mut conn)
    .expect("Failed to create user_config table");

    conn
}

#[test]
fn test_load_creates_default() {
    let mut conn = setup_diesel_conn();

    // First load should create default row
    let config = UserConfig::load_diesel(&mut conn).expect("Failed to load");

    assert_eq!(config.id, 1);
    assert_eq!(config.backend_mode, "native");
    assert!(!config.open_nested_in_new_tab);
}

#[test]
fn test_save_and_load() {
    let mut conn = setup_diesel_conn();

    // Create and load default
    let mut config = UserConfig::load_diesel(&mut conn).unwrap();

    // Modify
    config.backend_mode = "cli".to_string();
    config.socks5_enabled = true;
    config.socks5_address = Some("127.0.0.1:1080".to_string());

    // Save
    config.save_diesel(&mut conn).expect("Failed to save");

    // Reload
    let reloaded = UserConfig::load_diesel(&mut conn).unwrap();

    assert_eq!(reloaded.backend_mode, "cli");
    assert!(reloaded.socks5_enabled);
    assert_eq!(reloaded.socks5_address, Some("127.0.0.1:1080".to_string()));
}

#[test]
fn test_plugin_settings_json() {
    let mut config = UserConfig::new();

    // Set plugin settings
    let mut settings = std::collections::HashMap::new();
    settings.insert("key1".to_string(), "value1".to_string());
    settings.insert("key2".to_string(), "value2".to_string());

    config.set_plugin_settings("test_plugin", settings.clone());

    // Get plugin settings back
    let retrieved = config.get_plugin_settings("test_plugin");
    assert_eq!(retrieved.get("key1"), Some(&"value1".to_string()));
    assert_eq!(retrieved.get("key2"), Some(&"value2".to_string()));

    // Non-existent plugin should return empty
    let empty = config.get_plugin_settings("nonexistent");
    assert!(empty.is_empty());
}

#[test]
fn test_toolbar_order() {
    let mut config = UserConfig::new();

    let order = vec!["btn1".to_string(), "btn2".to_string(), "btn3".to_string()];
    config.set_toolbar_order(&order);

    let retrieved = config.get_toolbar_order();
    assert_eq!(retrieved, order);
}

#[test]
fn test_plugin_proxy_settings() {
    let mut config = UserConfig::new();

    config.set_plugin_proxy_enabled("plugin1", true);
    config.set_plugin_proxy_enabled("plugin2", false);

    let settings = config.get_plugin_proxy_settings();
    assert_eq!(settings.get("plugin1"), Some(&true));
    assert_eq!(settings.get("plugin2"), Some(&false));
}

#[test]
fn test_hotkey_bindings() {
    let mut config = UserConfig::new();

    // Set individual bindings
    config.set_hotkey_binding("navigate_back", r#"{"key":"Mouse4","modifiers":{}}"#);
    config.set_hotkey_binding("navigate_forward", r#"{"key":"Mouse5","modifiers":{}}"#);

    // Get individual binding
    let binding = config.get_hotkey_binding("navigate_back");
    assert!(binding.is_some());
    assert!(binding.unwrap().contains("Mouse4"));

    // Get all bindings
    let all_bindings = config.get_hotkey_bindings();
    assert_eq!(all_bindings.len(), 2);

    // Remove binding
    config.remove_hotkey_binding("navigate_forward");
    assert!(config.get_hotkey_binding("navigate_forward").is_none());
    assert_eq!(config.get_hotkey_bindings().len(), 1);
}

#[test]
fn test_hotkey_bindings_persistence() {
    let mut conn = setup_diesel_conn();

    // Load default config
    let mut config = UserConfig::load_diesel(&mut conn).unwrap();

    // Set hotkey binding
    config.set_hotkey_binding("open_archive", r#"{"key":"O","modifiers":{"ctrl":true}}"#);
    config.save_diesel(&mut conn).unwrap();

    // Reload and verify
    let reloaded = UserConfig::load_diesel(&mut conn).unwrap();
    let binding = reloaded.get_hotkey_binding("open_archive");
    assert!(binding.is_some());
    assert!(binding.unwrap().contains("ctrl"));
}

#[test]
fn test_gameta_server_fields_default() {
    let config = UserConfig::default();
    assert!(!config.gameta_server_enabled);
    assert!(config.gameta_server_url.is_none());
}
