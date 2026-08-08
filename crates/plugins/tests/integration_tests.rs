//! Integration tests for the plugin system
//!
//! These tests verify the end-to-end functionality of the plugin system.

use arclain_plugins::{PluginEvent, PluginLoader, PluginManager};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Helper to create a test plugins directory with a manifest
fn create_test_plugin_dir() -> TempDir {
    let temp_dir = TempDir::new().unwrap();
    let plugin_dir = temp_dir.path().join("test-plugin");
    fs::create_dir(&plugin_dir).unwrap();

    // Create a minimal plugin manifest
    let manifest = r#"
[plugin]
id = "test-plugin"
name = "Test Plugin"
version = "1.0.0"
author = "Test Author"
description = "A test plugin"

[capabilities]
network = false
file_read = false
file_write = false
archive_metadata_read = false
archive_metadata_write = false
archive_modify = false

[rate_limits]
http_requests_per_minute = 10
"#;

    fs::write(plugin_dir.join("plugin.toml"), manifest).unwrap();

    // Note: We would also need to create a .wasm file for full integration testing
    // For now, this tests the discovery and manifest loading

    temp_dir
}

fn bundled_plugins_dir() -> PathBuf {
    env::var_os("ARCLAIN_BUNDLED_PLUGIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(|path| path.parent())
                .expect("crates/plugins should live two levels below repo root")
                .join("plugins")
        })
}

#[test]
fn test_plugin_manager_creation() {
    let temp_dir = TempDir::new().unwrap();
    let manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new());

    assert!(manager.is_ok(), "Failed to create plugin manager");
}

#[test]
fn test_plugin_manager_init_empty_directory() {
    let temp_dir = TempDir::new().unwrap();
    let mut manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();

    // Initialize with empty directory should succeed
    let result = manager.init();
    assert!(
        result.is_ok(),
        "Failed to initialize plugin manager: {:?}",
        result
    );

    // Should have no plugins
    assert_eq!(manager.list_plugins().len(), 0);
}

#[test]
fn bundled_dlsite_plugin_loads_against_current_host() {
    // Override lets us point this same assertion at packaged plugin dirs
    // when diagnosing a user's local install.
    let loader = PluginLoader::new(bundled_plugins_dir()).unwrap();
    let discovered = loader.discover_plugins().unwrap();
    let dlsite = discovered
        .iter()
        .find(|plugin| plugin.manifest.plugin.id == "dlsite-metadata")
        .expect("bundled dlsite-metadata manifest should be discovered");
    let loaded = loader.load_plugin(dlsite).unwrap();
    let mut instance = loaded
        .instantiate(
            dlsite.manifest.capabilities.to_capabilities(),
            dlsite.manifest.rate_limits.http_requests_per_minute,
            HashMap::new(),
            None,
        )
        .unwrap();

    instance.init().unwrap();
    let metadata = instance.get_metadata().unwrap();
    assert_eq!(metadata.id, "dlsite-metadata");
}

#[test]
fn bundled_ui_demo_round_trips_through_wirt_world() {
    let loader = PluginLoader::new(bundled_plugins_dir()).unwrap();
    let discovered = loader.discover_plugins().unwrap();
    let demo = discovered
        .iter()
        .find(|plugin| plugin.manifest.plugin.id == "ui-demo")
        .expect("bundled ui-demo manifest should be discovered");
    let mut instance = loader
        .load_plugin(demo)
        .unwrap()
        .instantiate(
            demo.manifest.capabilities.to_capabilities(),
            demo.manifest.rate_limits.http_requests_per_minute,
            HashMap::new(),
            None,
        )
        .unwrap();

    instance.init().unwrap();
    assert_eq!(instance.get_metadata().unwrap().id, "ui-demo");
    let layout = instance
        .get_ui_layout(arclain_plugins::types::PluginExtensionPoint::MainPage)
        .unwrap();
    assert!(!layout.elements().is_empty());
    assert!(instance.send_ui_event("demo_btn", None).unwrap().is_empty());
}

#[test]
fn test_plugin_discovery() {
    let temp_dir = create_test_plugin_dir();
    let mut manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();

    // Note: This will fail to load the plugin since there's no .wasm file,
    // but it should discover the manifest
    let _ = manager.init();

    // The plugin won't be loaded (no .wasm), but discovery should work
    // In a full test, we'd create a valid .wasm file
}

#[test]
fn test_plugin_enable_disable() {
    let temp_dir = TempDir::new().unwrap();
    let mut manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();
    manager.init().unwrap();

    // Try to enable non-existent plugin
    let result = manager.enable_plugin("nonexistent");
    assert!(result.is_err(), "Should fail to enable nonexistent plugin");

    // Try to disable non-existent plugin
    let result = manager.disable_plugin("nonexistent");
    assert!(result.is_err(), "Should fail to disable nonexistent plugin");
}

#[test]
fn test_event_dispatch_empty() {
    let temp_dir = TempDir::new().unwrap();
    let mut manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();
    manager.init().unwrap();

    // Dispatch event to empty plugin set
    let event = PluginEvent::OnArchiveOpen {
        path: "test.zip".to_string(),
        kind: arclain_core::ArchiveKind::Zip,
        password: None,
        entries: std::sync::Arc::new(Vec::new()),
        archive_session_id: 1,
    };

    let responses = manager.dispatch_event(&event);
    assert_eq!(
        responses.len(),
        0,
        "Should have no responses from empty plugin set"
    );
}

#[test]
fn test_get_plugin_metadata_nonexistent() {
    let temp_dir = TempDir::new().unwrap();
    let manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();

    let metadata = manager.get_plugin_metadata("nonexistent");
    assert!(
        metadata.is_none(),
        "Should return None for nonexistent plugin"
    );
}

#[test]
fn test_plugins_directory_path() {
    let temp_dir = TempDir::new().unwrap();
    let expected_path = temp_dir.path().to_path_buf();
    let manager = PluginManager::new(expected_path.clone(), HashMap::new()).unwrap();

    assert_eq!(manager.plugins_dir(), expected_path.as_path());
}

#[test]
fn test_unload_nonexistent_plugin() {
    let temp_dir = TempDir::new().unwrap();
    let mut manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();
    manager.init().unwrap();

    let result = manager.unload_plugin("nonexistent");
    assert!(result.is_err(), "Should fail to unload nonexistent plugin");
}

#[test]
fn test_reload_nonexistent_plugin() {
    let temp_dir = TempDir::new().unwrap();
    let mut manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();
    manager.init().unwrap();

    let result = manager.reload_plugin("nonexistent");
    assert!(result.is_err(), "Should fail to reload nonexistent plugin");
}

#[test]
fn test_dispatch_to_specific_plugin() {
    let temp_dir = TempDir::new().unwrap();
    let mut manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();
    manager.init().unwrap();

    let event = PluginEvent::OnArchiveOpen {
        path: "test.zip".to_string(),
        kind: arclain_core::ArchiveKind::Zip,
        password: None,
        entries: std::sync::Arc::new(Vec::new()),
        archive_session_id: 1,
    };

    // Should fail for nonexistent plugin
    let result = manager.dispatch_event_to_plugin("nonexistent", &event);
    assert!(result.is_err(), "Should fail for nonexistent plugin");
}

#[test]
fn test_is_plugin_enabled_nonexistent() {
    let temp_dir = TempDir::new().unwrap();
    let manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();

    // Nonexistent plugin should return false
    assert!(!manager.is_plugin_enabled("nonexistent"));
}

#[test]
fn test_multiple_event_types() {
    let temp_dir = TempDir::new().unwrap();
    let mut manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();
    manager.init().unwrap();

    // Only `OnArchiveOpen` exists today; the dispatch_event call still
    // returns zero responses against an empty plugin set, which is the
    // invariant this test was guarding.
    let event = PluginEvent::OnArchiveOpen {
        path: "test.zip".to_string(),
        kind: arclain_core::ArchiveKind::Zip,
        password: None,
        entries: std::sync::Arc::new(Vec::new()),
        archive_session_id: 1,
    };
    let responses = manager.dispatch_event(&event);
    assert_eq!(
        responses.len(),
        0,
        "Should have no responses from empty plugin set"
    );
}
