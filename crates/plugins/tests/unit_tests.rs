//! Unit tests for plugin system components

use arclain_plugins::{
    PluginCapability, PluginError, PluginEvent, PluginMetadata, PluginResponse,
};

#[test]
fn test_plugin_capability_enum() {
    // Test that all capabilities can be created
    let caps = vec![
        PluginCapability::Network,
        PluginCapability::FileRead,
        PluginCapability::FileWrite,
        PluginCapability::ArchiveMetadataRead,
        PluginCapability::ArchiveMetadataWrite,
        PluginCapability::ArchiveModify,
    ];
    
    assert_eq!(caps.len(), 6, "Should have 6 capability types");
}

#[test]
fn test_plugin_metadata_creation() {
    let metadata = PluginMetadata {
        id: "test-plugin".to_string(),
        name: "Test Plugin".to_string(),
        version: "1.0.0".to_string(),
        author: "Test Author".to_string(),
        description: "A test plugin".to_string(),
    };
    
    assert_eq!(metadata.id, "test-plugin");
    assert_eq!(metadata.name, "Test Plugin");
    assert_eq!(metadata.version, "1.0.0");
    assert_eq!(metadata.author, "Test Author");
    assert_eq!(metadata.description, "A test plugin");
}

#[test]
fn test_plugin_error_types() {
    let errors = vec![
        PluginError::LoadError("test".to_string()),
        PluginError::InitError("test".to_string()),
        PluginError::ExecutionError("test".to_string()),
        PluginError::WasmError("test".to_string()),
        PluginError::InvalidManifest("test".to_string()),
        PluginError::NotFound("test".to_string()),
        PluginError::CapabilityDenied(PluginCapability::Network),
    ];
    
    assert_eq!(errors.len(), 7, "Should have 7 error types");
}

#[test]
fn test_plugin_event_types() {
    let events = vec![
        PluginEvent::OnArchiveOpen {
            path: "test.zip".to_string(),
            kind: arclain_core::ArchiveKind::Zip,
        },
        PluginEvent::OnArchiveClose {
            path: "test.zip".to_string(),
        },
        PluginEvent::OnFileExtract {
            archive: "test.zip".to_string(),
            file_path: "file.txt".to_string(),
        },
        PluginEvent::OnMetadataDisplay {
            archive: "test.zip".to_string(),
        },
    ];
    
    assert_eq!(events.len(), 4, "Should have 4 event types");
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
        assert_eq!(manifests.len(), 0, "Should find no plugins in empty directory");
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
[plugin]
id = "test-plugin"
name = "Test Plugin"
version = "1.0.0"
author = "Test Author"
description = "A test plugin"

[capabilities]
network = true
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
    use arclain_plugins::host_functions::{HostFunctions, RateLimiter};
    use arclain_plugins::PluginCapability;
    use std::collections::HashSet;

    #[test]
    fn test_rate_limiter_creation() {
        let limiter = RateLimiter::new(10);
        
        // Should allow first 10 requests
        for _ in 0..10 {
            assert!(limiter.check_rate_limit(), "Should allow request");
        }
        
        // Should deny 11th request
        assert!(!limiter.check_rate_limit(), "Should deny request after limit");
    }

    #[test]
    fn test_host_functions_creation() {
        let mut caps = HashSet::new();
        caps.insert(PluginCapability::Network);
        
        let host_funcs = HostFunctions::new(caps, 10);
        
        assert!(host_funcs.http_client.is_some(), "Should have HTTP client");
        assert!(host_funcs.check_capability(PluginCapability::Network));
        assert!(!host_funcs.check_capability(PluginCapability::FileRead));
    }

    #[test]
    fn test_host_functions_no_network() {
        let caps = HashSet::new();
        let host_funcs = HostFunctions::new(caps, 10);
        
        assert!(host_funcs.http_client.is_none(), "Should not have HTTP client");
        assert!(!host_funcs.check_capability(PluginCapability::Network));
    }

    #[test]
    fn test_buffer_allocation() {
        let caps = HashSet::new();
        let host_funcs = HostFunctions::new(caps, 10);
        
        let data = vec![1, 2, 3, 4, 5];
        let id = host_funcs.allocate_buffer(data.clone());
        
        assert!(id > 0, "Should allocate non-zero buffer ID");
        
        let retrieved = host_funcs.take_buffer(id);
        assert_eq!(retrieved, Some(data), "Should retrieve same data");
        
        // Should be removed after taking
        assert_eq!(host_funcs.take_buffer(id), None, "Buffer should be removed");
    }

    #[test]
    fn test_multiple_buffer_allocations() {
        let caps = HashSet::new();
        let host_funcs = HostFunctions::new(caps, 10);
        
        let data1 = vec![1, 2, 3];
        let data2 = vec![4, 5, 6];
        
        let id1 = host_funcs.allocate_buffer(data1.clone());
        let id2 = host_funcs.allocate_buffer(data2.clone());
        
        assert_ne!(id1, id2, "Should allocate different IDs");
        
        assert_eq!(host_funcs.take_buffer(id1), Some(data1));
        assert_eq!(host_funcs.take_buffer(id2), Some(data2));
    }
}