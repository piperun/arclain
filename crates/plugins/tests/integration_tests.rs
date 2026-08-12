//! Integration tests for the plugin system
//!
//! These tests verify the end-to-end functionality of the plugin system.

use arclain_plugins::{PluginEvent, PluginManager};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
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
    if let Some(directory) = env::var_os("ARCLAIN_BUNDLED_PLUGIN_DIR") {
        return PathBuf::from(directory);
    }

    static FLAT_PACKAGES: OnceLock<TempDir> = OnceLock::new();
    let packages = FLAT_PACKAGES.get_or_init(|| {
        let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|path| path.parent())
            .expect("crates/plugins should live two levels below repo root")
            .to_path_buf();
        let directory = TempDir::new().expect("create flat bundled-package fixture");
        for plugin_id in ["dlsite-metadata", "ui-demo"] {
            let manifest = fs::read(
                repository_root
                    .join("plugins")
                    .join(plugin_id)
                    .join("plugin.toml"),
            )
            .expect("read bundled plugin manifest");
            let component = fs::read(
                repository_root
                    .join("crates/wirt/tests/fixtures/bundled")
                    .join(format!("{plugin_id}.wasm")),
            )
            .expect("read tracked bundled component fixture");
            let package = wirt::package_bytes(&manifest, &component)
                .expect("construct validated bundled Wirt package");
            fs::write(directory.path().join(format!("{plugin_id}.wirt")), package)
                .expect("write bundled Wirt package fixture");
        }
        assert!(
            fs::read_dir(directory.path()).unwrap().all(|entry| {
                entry
                    .unwrap()
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("wirt"))
            }),
            "bundled fixture must contain only .wirt packages"
        );
        directory
    });
    packages.path().to_path_buf()
}

fn fixture_package(temp_dir: &TempDir, plugin_id: &str) -> (PathBuf, wirt::PackageFingerprint) {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repository_root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("crates/plugins should live two levels below repo root");
    let manifest = fs::read(
        repository_root
            .join("plugins")
            .join(plugin_id)
            .join("plugin.toml"),
    )
    .expect("read plugin manifest fixture");
    let component = if plugin_id == "ui-demo" {
        fs::read(
            repository_root
                .join("crates/wirt/tests/fixtures/bundled")
                .join(format!("{plugin_id}.wasm")),
        )
    } else {
        fs::read(
            repository_root
                .join("plugins")
                .join(plugin_id)
                .join(format!("{plugin_id}.wasm")),
        )
    }
    .expect("read plugin component fixture");
    let package = wirt::package_bytes(&manifest, &component).expect("build plugin package fixture");
    let fingerprint = wirt::PackageFingerprint::sha256(&package);
    let package_path = temp_dir.path().join(format!("{plugin_id}.wirt"));
    fs::write(&package_path, package).expect("write plugin package fixture");
    (package_path, fingerprint)
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
    let mut manager = PluginManager::new(bundled_plugins_dir(), HashMap::new()).unwrap();
    manager.init().unwrap();
    let metadata = manager
        .execute_plugin("dlsite-metadata", wirt::ExecutorRequest::Metadata)
        .unwrap()
        .into_metadata()
        .unwrap();
    assert_eq!(metadata.id, "dlsite-metadata");
}

#[test]
fn bundled_ui_demo_round_trips_through_wirt_world() {
    let mut manager = PluginManager::new(bundled_plugins_dir(), HashMap::new()).unwrap();
    manager.init().unwrap();
    assert_eq!(
        manager
            .execute_plugin("ui-demo", wirt::ExecutorRequest::Metadata)
            .unwrap()
            .into_metadata()
            .unwrap()
            .id,
        "ui-demo"
    );
    let layout = manager
        .execute_plugin(
            "ui-demo",
            wirt::ExecutorRequest::UiLayout {
                extension_point: wirt::PluginExtensionPoint::MainPage,
            },
        )
        .unwrap();
    let layout = layout.into_layout().unwrap();
    assert!(!layout.elements().is_empty());
    assert!(manager
        .execute_plugin(
            "ui-demo",
            wirt::ExecutorRequest::UiEvent {
                id: "demo_btn".to_string(),
                value: None,
            },
        )
        .unwrap()
        .into_actions()
        .unwrap()
        .is_empty());
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

#[test]
fn uninstalling_a_disabled_package_removes_artifact_registry_routing_and_quarantine() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let (package_path, fingerprint) = fixture_package(&temp_dir, "facade-test-fixture");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let whitelist = std::sync::Arc::new(parking_lot::RwLock::new(
        arclain_network::features::whitelist::DomainWhitelist::default(),
    ));
    let client = std::sync::Arc::new(arclain_network::AsyncHttpClient::new(
        runtime.handle().clone(),
        whitelist,
        None,
    ));
    let mut manager = PluginManager::new(
        plugins_dir.clone(),
        HashMap::from([(
            "facade-test-fixture".to_string(),
            HashMap::from([("seed".to_string(), "retained".to_string())]),
        )]),
    )
    .unwrap();
    manager.set_async_http_client(client.clone());
    manager
        .install_plugin_package(&package_path, &fingerprint)
        .unwrap();
    let installed_dir = plugins_dir.join("facade-test-fixture");
    assert!(installed_dir.join("facade-test-fixture.wasm").is_file());
    assert!(installed_dir.join("facade-test-fixture.toml").is_file());
    assert!(installed_dir.join("package.sha256").is_file());
    assert!(client
        .plugin_network_policy("facade-test-fixture")
        .is_some());

    let quota_error = manager
        .execute_plugin(
            "facade-test-fixture",
            wirt::ExecutorRequest::UiEvent {
                id: "trigger-result-quota".to_string(),
                value: None,
            },
        )
        .expect_err("fixture action must quarantine and disable the plugin");
    assert!(matches!(
        quota_error,
        arclain_plugins::PluginError::ResourceLimit { .. }
    ));

    manager
        .uninstall_package("facade-test-fixture")
        .expect("disabled package should uninstall");

    assert!(!installed_dir.exists());
    assert!(manager.list_plugins().is_empty());
    assert!(manager.get_settings_for("facade-test-fixture").is_none());
    assert!(client
        .plugin_network_policy("facade-test-fixture")
        .is_none());

    manager
        .install_plugin_package(&package_path, &fingerprint)
        .expect("the same package should reinstall without stale quarantine");
    let reinstalled = manager
        .list_plugins()
        .into_iter()
        .find(|plugin| plugin.id == "facade-test-fixture")
        .unwrap();
    assert!(matches!(
        reinstalled.quarantine_state,
        arclain_plugins::QuarantineState::Clear
    ));
}

#[test]
fn uninstalling_an_enabled_plugin_conflicts_without_mutation() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let (package_path, fingerprint) = fixture_package(&temp_dir, "ui-demo");
    let mut manager = PluginManager::new(plugins_dir.clone(), HashMap::new()).unwrap();
    manager
        .install_plugin_package(&package_path, &fingerprint)
        .unwrap();

    let error = manager
        .uninstall_package("ui-demo")
        .expect_err("enabled plugins must be disabled before uninstall");

    assert!(matches!(error, arclain_plugins::PluginError::Conflict(_)));
    assert!(plugins_dir.join("ui-demo/package.sha256").is_file());
    assert!(manager.is_plugin_enabled("ui-demo"));
    assert_eq!(
        manager
            .execute_plugin("ui-demo", wirt::ExecutorRequest::Metadata)
            .unwrap()
            .into_metadata()
            .unwrap()
            .id,
        "ui-demo"
    );
}

#[test]
fn uninstall_deletion_failure_keeps_the_disabled_generation_registered_and_usable() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let (package_path, fingerprint) = fixture_package(&temp_dir, "ui-demo");
    let mut manager = PluginManager::new(plugins_dir.clone(), HashMap::new()).unwrap();
    manager
        .install_plugin_package(&package_path, &fingerprint)
        .unwrap();
    manager.disable_plugin("ui-demo").unwrap();
    let installed_dir = plugins_dir.join("ui-demo");
    fs::write(installed_dir.join("attacker-owned"), b"preserve").unwrap();

    manager
        .uninstall_package("ui-demo")
        .expect_err("unexpected artifact contents must fail closed");

    assert_eq!(
        fs::read(installed_dir.join("attacker-owned")).unwrap(),
        b"preserve"
    );
    assert!(installed_dir.join("ui-demo.wasm").is_file());
    assert!(installed_dir.join("ui-demo.toml").is_file());
    assert!(installed_dir.join("package.sha256").is_file());
    assert_eq!(manager.list_plugins().len(), 1);
    manager.enable_plugin("ui-demo").unwrap();
    assert_eq!(
        manager
            .execute_plugin("ui-demo", wirt::ExecutorRequest::Metadata)
            .unwrap()
            .into_metadata()
            .unwrap()
            .id,
        "ui-demo"
    );
}
