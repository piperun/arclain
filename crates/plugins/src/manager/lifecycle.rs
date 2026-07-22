//! Plugin lifecycle management (loading, unloading, installation)

use super::types::ManagedPlugin;
use super::PluginManager;
use crate::loader::DiscoveredPlugin;
use crate::types::{PluginError, PluginId, PluginMetadata, Result};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info};

impl PluginManager {
    /// Initialize the plugin manager and load discovered plugins
    pub fn init(&mut self) -> Result<()> {
        let plugins = self.loader.discover_plugins()?;
        for plugin in plugins {
            match self.load_plugin(&plugin) {
                Ok(_) => debug!("Loaded plugin: {}", plugin.manifest.plugin.id),
                Err(e) => error!("Failed to load plugin {}: {}", plugin.manifest.plugin.id, e),
            }
        }
        Ok(())
    }

    /// Load a single plugin
    pub(crate) fn load_plugin(&mut self, discovered: &DiscoveredPlugin) -> Result<()> {
        let plugin_id = discovered.manifest.plugin.id.clone();

        // Check if already loaded
        if self.plugins.read().contains_key(&plugin_id) {
            return Err(PluginError::LoadError(format!(
                "Plugin already loaded: {}",
                plugin_id
            )));
        }

        // Load the WASM module
        let loaded = self.loader.load_plugin(discovered)?;

        // Get capabilities from manifest
        let capabilities = discovered.manifest.capabilities.to_capabilities();

        // Get rate limit from manifest
        let rate_limit = discovered.manifest.rate_limits.http_requests_per_minute;

        // Get initial settings for this plugin
        let settings = self
            .initial_settings
            .get(&plugin_id)
            .cloned()
            .unwrap_or_default();

        // Instantiate the plugin with its host-function state.
        let mut instance = loaded.instantiate(
            capabilities.clone(),
            rate_limit,
            self.library_service.clone(),
            settings,
            self.active_tab_bridge.clone(),
        )?;

        // Inject optional services
        if let Some(ref client) = self.gameta_client {
            instance.set_gameta_client(Some(client.clone()));
        }

        // Initialize the plugin
        instance.init()?;

        // Get metadata from manifest (WIT get_metadata is not yet implemented and returns defaults)
        let manifest = &discovered.manifest;
        let metadata = PluginMetadata {
            id: manifest.plugin.id.clone(),
            name: manifest.plugin.name.clone(),
            version: manifest.plugin.version.clone(),
            description: manifest.plugin.description.clone(),
            author: manifest.plugin.author.clone(),
        };

        // Snapshot the dirty handle BEFORE moving the instance into the
        // Arc<Mutex<...>> — saves a redundant lock just to clone an Arc.
        let settings_dirty = instance.settings_dirty_handle();

        // Create managed plugin
        let managed = ManagedPlugin {
            metadata: metadata.clone(),
            instance: Arc::new(Mutex::new(instance)),
            manifest: discovered.manifest.clone(),
            enabled: true,
            settings_dirty,
        };

        // Store plugin
        self.plugins.write().insert(plugin_id.clone(), managed);
        self.enabled_plugins.write().insert(plugin_id.clone(), true);
        self.invalidate_top_tabs_cache();

        // Auto-approve network domains from manifest
        if !discovered.manifest.capabilities.network_domains.is_empty() {
            if let Some(ref client) = self.async_http_client {
                for domain in &discovered.manifest.capabilities.network_domains {
                    client.approve_domain(&plugin_id, domain);
                }
            }
        }

        info!("Plugin '{}' loaded and initialized", metadata.name);
        Ok(())
    }

    /// Reload a plugin
    pub fn reload_plugin(&mut self, plugin_id: &str) -> Result<()> {
        info!("Reloading plugin: {}", plugin_id);

        // Remove existing plugin
        self.plugins.write().remove(plugin_id);

        // Discover plugins again
        let discovered = self.loader.discover_plugins()?;

        // Find the plugin to reload
        let plugin_info = discovered
            .iter()
            .find(|p| p.manifest.plugin.id == plugin_id)
            .ok_or_else(|| PluginError::NotFound(plugin_id.to_string()))?;

        // Load it
        self.load_plugin(plugin_info)?;

        info!("Plugin reloaded: {}", plugin_id);
        Ok(())
    }

    /// Unload a plugin
    pub fn unload_plugin(&mut self, plugin_id: &str) -> Result<()> {
        info!("Unloading plugin: {}", plugin_id);

        let mut plugins = self.plugins.write();

        if let Some(plugin) = plugins.remove(plugin_id) {
            plugin.instance.lock().cleanup()?;
            drop(plugins);
            self.invalidate_top_tabs_cache();
            info!("Plugin unloaded: {}", plugin_id);
            Ok(())
        } else {
            Err(PluginError::NotFound(plugin_id.to_string()))
        }
    }

    /// Install a plugin from a .wasm file
    ///
    /// This will:
    /// 1. Load and validate the plugin from the .wasm file
    /// 2. Extract metadata and create a manifest
    /// 3. Create a directory in plugins/ with the plugin ID
    /// 4. Copy the .wasm file and create plugin.toml
    /// 5. Load the plugin into the manager
    pub fn install_plugin(&mut self, wasm_path: &std::path::Path) -> Result<String> {
        use std::fs;
        use std::io::Write;

        info!("Installing plugin from: {}", wasm_path.display());

        // Validate file exists and is a .wasm file
        if !wasm_path.exists() {
            return Err(PluginError::LoadError("File does not exist".to_string()));
        }

        if wasm_path.extension().and_then(|s| s.to_str()) != Some("wasm") {
            return Err(PluginError::LoadError(
                "File must be a .wasm file".to_string(),
            ));
        }

        // Read WASM file
        let wasm_bytes = fs::read(wasm_path)
            .map_err(|e| PluginError::LoadError(format!("Failed to read WASM file: {}", e)))?;

        // Load the plugin to get metadata (without full instantiation)
        let loaded = self.loader.load_wasm(&wasm_bytes)?;

        // Create a temporary instance to get metadata
        let capabilities = Vec::new(); // Empty capabilities for validation
        let mut temp_instance =
            loaded.instantiate(capabilities, 60, None, HashMap::new(), None)?;
        temp_instance.init()?;
        let metadata = temp_instance.get_metadata()?;
        temp_instance.cleanup()?;

        let plugin_id = PluginId::parse(metadata.id.clone())?;

        // Check if plugin is already installed
        if self.plugins.read().contains_key(plugin_id.as_str()) {
            return Err(PluginError::LoadError(format!(
                "Plugin '{}' is already installed",
                plugin_id
            )));
        }

        // Create plugin directory
        let plugin_dir = plugin_id.join_under(self.plugins_dir());
        fs::create_dir_all(&plugin_dir).map_err(|e| {
            PluginError::LoadError(format!("Failed to create plugin directory: {}", e))
        })?;

        // Copy WASM file
        let wasm_dest = plugin_dir.join("plugin.wasm");
        fs::copy(wasm_path, &wasm_dest)
            .map_err(|e| PluginError::LoadError(format!("Failed to copy WASM file: {}", e)))?;

        // Create manifest from metadata
        let manifest_content = format!(
            r#"[plugin]
id = "{}"
name = "{}"
version = "{}"
description = "{}"
author = "{}"

[capabilities]
file_read = false
file_write = false
network = false
http_requests = false

[rate_limits]
http_requests_per_minute = 60
"#,
            plugin_id.as_str(),
            metadata.name,
            metadata.version,
            metadata.description,
            metadata.author
        );

        let manifest_path = plugin_dir.join("plugin.toml");
        let mut manifest_file = fs::File::create(&manifest_path)
            .map_err(|e| PluginError::LoadError(format!("Failed to create manifest: {}", e)))?;
        manifest_file
            .write_all(manifest_content.as_bytes())
            .map_err(|e| PluginError::LoadError(format!("Failed to write manifest: {}", e)))?;

        info!("Plugin files installed to: {}", plugin_dir.display());

        // Reload plugins to pick up the new one
        let discovered = self.loader.discover_plugins()?;
        let plugin_info = discovered
            .iter()
            .find(|p| p.manifest.plugin.id == plugin_id.as_str())
            .ok_or_else(|| {
                PluginError::LoadError("Failed to discover newly installed plugin".to_string())
            })?;

        // Load the plugin
        self.load_plugin(plugin_info)?;

        info!("Plugin '{}' installed and loaded successfully", plugin_id);
        Ok(plugin_id.as_str().to_owned())
    }
}
