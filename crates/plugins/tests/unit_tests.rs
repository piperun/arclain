//! Unit tests for plugin system components

use arclain_plugins::{PluginEvent, PluginResponse};

#[test]
fn test_plugin_event_types() {
    // Only `OnArchiveOpen` is wired through the dispatch worker today;
    // other lifecycle variants were dropped in the 2026-05-19 audit
    // because the worker silently ignored them. See PluginEvent docs.
    let events = vec![PluginEvent::OnArchiveOpen {
        path: "test.zip".to_string(),
        kind: arclain_core::ArchiveKind::Zip,
        password: None,
        entries: std::sync::Arc::new(Vec::new()),
        archive_session_id: 1,
    }];

    assert_eq!(events.len(), 1, "Only OnArchiveOpen is currently supported");
}

#[test]
fn test_plugin_response_none() {
    let response = PluginResponse::None;
    assert!(matches!(response, PluginResponse::None));
}

#[test]
fn test_plugin_response_metadata() {
    let metadata = serde_json::json!({
        "title": "Test",
        "author": "Test Author"
    });

    let response = PluginResponse::Metadata {
        data: metadata.clone(),
    };

    if let PluginResponse::Metadata { data } = response {
        assert_eq!(data["title"], "Test");
        assert_eq!(data["author"], "Test Author");
    } else {
        panic!("Expected Metadata response");
    }
}

#[test]
fn test_plugin_response_error() {
    let response = PluginResponse::Error {
        message: "Test error".to_string(),
    };

    if let PluginResponse::Error { message } = response {
        assert_eq!(message, "Test error");
    } else {
        panic!("Expected Error response");
    }
}

mod runtime_tests {
    use arclain_plugins::WasmRuntime;

    #[test]
    fn test_wasm_runtime_creation() {
        let runtime = WasmRuntime::new();
        assert!(runtime.is_ok(), "Failed to create WASM runtime");
    }
}

mod loader_tests {
    use arclain_plugins::PluginLoader;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_loader_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

        let manifests = loader.discover_plugins().unwrap();
        assert_eq!(
            manifests.len(),
            0,
            "Should find no plugins in empty directory"
        );
    }

    #[test]
    fn test_loader_with_invalid_toml() {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path().join("test-plugin");
        fs::create_dir(&plugin_dir).unwrap();

        // Create invalid TOML
        fs::write(plugin_dir.join("test-plugin.toml"), "invalid toml {").unwrap();

        let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();
        let result = loader.discover_plugins();

        // Should handle error gracefully
        assert!(result.is_ok(), "Should handle invalid TOML gracefully");
    }

    #[test]
    fn test_loader_with_valid_manifest() {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path().join("test-plugin");
        fs::create_dir(&plugin_dir).unwrap();

        let manifest = r#"
[wirt]
abi = "0.1.0"

[plugin]
id = "test-plugin"
name = "Test Plugin"
version = "1.0.0"
author = "Test Author"
description = "A test plugin"

[capabilities]
network = true
network_domains = ["example.invalid"]
file_read = false

[rate_limits]
http_requests_per_minute = 10
"#;
        fs::write(plugin_dir.join("test-plugin.toml"), manifest).unwrap();

        // Create a dummy .wasm file (empty is fine for discovery test)
        fs::write(plugin_dir.join("test-plugin.wasm"), &[]).unwrap();

        let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();
        let manifests = loader.discover_plugins().unwrap();

        assert_eq!(manifests.len(), 1, "Should find one plugin");
        assert_eq!(manifests[0].manifest.plugin.id, "test-plugin");
        assert_eq!(manifests[0].manifest.plugin.name, "Test Plugin");
    }
}

mod host_functions_tests {
    // Note: Buffer allocation tests removed - Component Model handles memory automatically
}
