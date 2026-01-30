use super::*;
use tempfile::TempDir;

#[test]
fn test_plugin_loader_creation() {
    let temp_dir = TempDir::new().unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf());
    assert!(loader.is_ok());
}

#[test]
fn test_manifest_validation() {
    let temp_dir = TempDir::new().unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    let manifest = PluginManifest {
        plugin: crate::types::PluginInfoConfig {
            id: "test-plugin".to_string(),
            name: "Test Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: "Test Author".to_string(),
            description: "A test plugin".to_string(),
        },
        capabilities: crate::types::CapabilitiesConfig {
            network: false,
            network_domains: vec![],
            archive_metadata_read: true,
            archive_metadata_write: false,
            archive_modify: false,
            file_read: false,
            file_write: false,
        },
        rate_limits: Default::default(),
    };

    assert!(loader.validate_manifest(&manifest).is_ok());
}

#[test]
fn test_invalid_manifest() {
    let temp_dir = TempDir::new().unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    let manifest = PluginManifest {
        plugin: crate::types::PluginInfoConfig {
            id: "".to_string(), // Empty ID
            name: "Test Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: "Test Author".to_string(),
            description: "A test plugin".to_string(),
        },
        capabilities: Default::default(),
        rate_limits: Default::default(),
    };

    assert!(loader.validate_manifest(&manifest).is_err());
}
