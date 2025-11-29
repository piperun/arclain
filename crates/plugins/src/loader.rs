//! Plugin discovery and loading

use crate::runtime::{LoadedPlugin, WasmRuntime};
use crate::types::{PluginError, PluginInfo, PluginManifest, Result};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Discovers and loads plugins from a directory
pub struct PluginLoader {
    plugins_dir: PathBuf,
    runtime: WasmRuntime,
}

impl PluginLoader {
    /// Create a new plugin loader
    pub fn new(plugins_dir: PathBuf) -> Result<Self> {
        let runtime = WasmRuntime::new()?;
        
        // Create plugins directory if it doesn't exist
        if !plugins_dir.exists() {
            std::fs::create_dir_all(&plugins_dir)?;
            info!("Created plugins directory: {}", plugins_dir.display());
        }
        
        Ok(Self {
            plugins_dir,
            runtime,
        })
    }
    
    /// Discover all plugins in the plugins directory
    /// Supports two discovery modes:
    /// 1. Folder mode (preferred): plugins/plugin-name/plugin-name.toml + plugin-name.wasm
    /// 2. Flat mode (legacy): plugins/plugin-name.toml + plugin-name.wasm
    pub fn discover_plugins(&self) -> Result<Vec<DiscoveredPlugin>> {
        info!("Discovering plugins in: {}", self.plugins_dir.display());
        
        let mut discovered = Vec::new();
        
        for entry in std::fs::read_dir(&self.plugins_dir)? {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_dir() {
                // Folder mode: Look for manifest inside directory
                if let Some(plugin_name) = path.file_name().and_then(|n| n.to_str()) {
                    let manifest_path = path.join(format!("{}.toml", plugin_name));
                    
                    if manifest_path.exists() {
                        match self.discover_plugin_from_folder(&manifest_path) {
                            Ok(plugin) => {
                                debug!("Discovered plugin (folder): {} v{}", plugin.manifest.plugin.name, plugin.manifest.plugin.version);
                                discovered.push(plugin);
                            }
                            Err(e) => {
                                warn!("Failed to discover plugin in folder {}: {}", path.display(), e);
                            }
                        }
                    }
                }
            } else if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                // Flat mode: Look for .toml manifest files in plugins root
                match self.discover_plugin_flat(&path) {
                    Ok(plugin) => {
                        debug!("Discovered plugin (flat): {} v{}", plugin.manifest.plugin.name, plugin.manifest.plugin.version);
                        discovered.push(plugin);
                    }
                    Err(e) => {
                        warn!("Failed to discover plugin at {}: {}", path.display(), e);
                    }
                }
            }
        }
        
        info!("Discovered {} plugins", discovered.len());
        Ok(discovered)
    }
    
    /// Discover plugin from folder structure (preferred)
    /// Expected structure: plugins/plugin-name/plugin-name.toml + plugin-name.wasm
    fn discover_plugin_from_folder(&self, manifest_path: &Path) -> Result<DiscoveredPlugin> {
        // Read and parse manifest
        let manifest_content = std::fs::read_to_string(manifest_path)?;
        let manifest: PluginManifest = toml::from_str(&manifest_content)?;
        
        // Validate manifest
        self.validate_manifest(&manifest)?;
        
        // Find corresponding .wasm file in the same directory
        let wasm_path = manifest_path.with_extension("wasm");
        if !wasm_path.exists() {
            return Err(PluginError::LoadError(format!(
                "WASM file not found: {}",
                wasm_path.display()
            )));
        }
        
        debug!("Found plugin WASM: {}", wasm_path.display());
        
        Ok(DiscoveredPlugin {
            manifest,
            manifest_path: manifest_path.to_path_buf(),
            wasm_path,
        })
    }
    
    /// Discover plugin from flat structure (legacy compatibility)
    /// Expected structure: plugins/plugin-name.toml + plugins/plugin-name.wasm
    fn discover_plugin_flat(&self, manifest_path: &Path) -> Result<DiscoveredPlugin> {
        // Read and parse manifest
        let manifest_content = std::fs::read_to_string(manifest_path)?;
        let manifest: PluginManifest = toml::from_str(&manifest_content)?;
        
        // Validate manifest
        self.validate_manifest(&manifest)?;
        
        // Find corresponding .wasm file
        let wasm_path = manifest_path.with_extension("wasm");
        if !wasm_path.exists() {
            return Err(PluginError::LoadError(format!(
                "WASM file not found: {}",
                wasm_path.display()
            )));
        }
        
        debug!("Found plugin WASM: {}", wasm_path.display());
        
        Ok(DiscoveredPlugin {
            manifest,
            manifest_path: manifest_path.to_path_buf(),
            wasm_path,
        })
    }
    
    /// Validate a plugin manifest
    pub fn validate_manifest(&self, manifest: &PluginManifest) -> Result<()> {
        // Check required fields
        if manifest.plugin.id.is_empty() {
            return Err(PluginError::InvalidManifest("Plugin ID is empty".to_string()));
        }
        
        if manifest.plugin.name.is_empty() {
            return Err(PluginError::InvalidManifest("Plugin name is empty".to_string()));
        }
        
        if manifest.plugin.version.is_empty() {
            return Err(PluginError::InvalidManifest("Plugin version is empty".to_string()));
        }
        
        // Validate ID format (alphanumeric and hyphens only)
        if !manifest.plugin.id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return Err(PluginError::InvalidManifest(
                "Plugin ID must contain only alphanumeric characters, hyphens, and underscores".to_string()
            ));
        }
        
        Ok(())
    }
    
    /// Load a plugin from a discovered plugin
    pub fn load_plugin(&self, discovered: &DiscoveredPlugin) -> Result<LoadedPlugin> {
        info!("Loading plugin: {}", discovered.manifest.plugin.name);
        
        let loaded = self.runtime.load_module(&discovered.wasm_path)?;
        
        info!("Plugin loaded successfully: {}", discovered.manifest.plugin.name);
        Ok(loaded)
    }
    
    /// Load a plugin directly from WASM bytes
    /// This is used for plugin installation to validate the plugin before copying files
    pub fn load_wasm(&self, wasm_bytes: &[u8]) -> Result<LoadedPlugin> {
        self.runtime.load_module_from_bytes(wasm_bytes)
    }
    
    /// Get the plugins directory path
    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }
}

/// A discovered plugin with its manifest and paths
#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    pub manifest: PluginManifest,
    pub manifest_path: PathBuf,
    pub wasm_path: PathBuf,
}

impl DiscoveredPlugin {
    /// Get plugin info for display
    pub fn to_plugin_info(&self) -> PluginInfo {
        PluginInfo {
            metadata: crate::types::PluginMetadata {
                id: self.manifest.plugin.id.clone(),
                name: self.manifest.plugin.name.clone(),
                version: self.manifest.plugin.version.clone(),
                author: self.manifest.plugin.author.clone(),
                description: self.manifest.plugin.description.clone(),
            },
            capabilities: self.manifest.capabilities.to_capabilities(),
            manifest_path: self.manifest_path.clone(),
            wasm_path: self.wasm_path.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
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
}