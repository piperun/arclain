//! Plugin manager for lifecycle and event dispatching

use crate::loader::{DiscoveredPlugin, PluginLoader};
use crate::runtime::PluginInstance;
use crate::types::{PluginError, PluginEvent, PluginMetadata, PluginResponse, Result};
use arclain_core::sevenzip::SevenZipCli;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, error, info};

/// Information about a plugin for UI display
#[derive(Clone, Debug)]
pub struct PluginListItem {
    pub id: String,
    pub manifest: crate::types::PluginManifest,
    pub enabled: bool,
    pub instance: Option<()>, // Just a marker for whether it's loaded
}

/// Manages all loaded plugins and dispatches events
pub struct PluginManager {
    loader: PluginLoader,
    plugins: Arc<RwLock<HashMap<String, ManagedPlugin>>>,
    enabled_plugins: Arc<RwLock<HashMap<String, bool>>>,
    backend: Option<Arc<SevenZipCli>>,
    metadata_cache: Option<Arc<arclain_db::MetadataCache>>,
}

impl PluginManager {
    /// Create a new plugin manager
    pub fn new(plugins_dir: PathBuf) -> Result<Self> {
        let loader = PluginLoader::new(plugins_dir)?;

        Ok(Self {
            loader,
            plugins: Arc::new(RwLock::new(HashMap::new())),
            enabled_plugins: Arc::new(RwLock::new(HashMap::new())),
            backend: None,
            metadata_cache: None,
        })
    }

    /// Create a new plugin manager with archive backend
    pub fn with_backend(plugins_dir: PathBuf, backend: Arc<SevenZipCli>) -> Result<Self> {
        let loader = PluginLoader::new(plugins_dir)?;

        Ok(Self {
            loader,
            plugins: Arc::new(RwLock::new(HashMap::new())),
            enabled_plugins: Arc::new(RwLock::new(HashMap::new())),
            backend: Some(backend),
            metadata_cache: None,
        })
    }

    /// Set the archive backend for file operations
    pub fn set_backend(&mut self, backend: Arc<SevenZipCli>) {
        self.backend = Some(backend);
    }

    /// Set the metadata cache and propagate to all loaded plugins
    pub fn set_metadata_cache(&mut self, cache: Arc<arclain_db::MetadataCache>) {
        self.metadata_cache = Some(cache.clone());

        // Propagate to all existing plugin instances
        let mut plugins = self.plugins.write();
        for plugin in plugins.values_mut() {
            plugin.instance.set_metadata_cache(Some(cache.clone()));
        }
    }

    /// Set the current archive context for all plugins
    pub fn set_archive_context(&self, archive_path: Option<String>, password: Option<String>) {
        let mut plugins = self.plugins.write();
        for plugin in plugins.values_mut() {
            plugin
                .instance
                .set_archive_context(archive_path.clone(), password.clone());
        }
    }

    /// Initialize and load all plugins
    pub fn init(&mut self) -> Result<()> {
        info!("Initializing plugin system");

        let discovered = self.loader.discover_plugins()?;

        for plugin_info in discovered {
            match self.load_plugin(&plugin_info) {
                Ok(()) => {
                    info!("Plugin loaded: {}", plugin_info.manifest.plugin.id);
                }
                Err(e) => {
                    error!(
                        "Failed to load plugin {}: {}",
                        plugin_info.manifest.plugin.id, e
                    );
                }
            }
        }

        info!(
            "Plugin system initialized with {} plugins",
            self.plugins.read().len()
        );
        Ok(())
    }

    /// Load a single plugin
    fn load_plugin(&mut self, discovered: &DiscoveredPlugin) -> Result<()> {
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

        // Instantiate the plugin with backend if available
        let mut instance = if let Some(ref backend) = self.backend {
            loaded.instantiate_with_backend(
                capabilities.clone(),
                rate_limit,
                Some(backend.clone()),
                self.metadata_cache.clone(),
            )?
        } else {
            loaded.instantiate_with_backend(
                capabilities.clone(),
                rate_limit,
                None,
                self.metadata_cache.clone(),
            )?
        };

        // Initialize the plugin
        instance.init()?;

        // Get metadata
        let metadata = instance.get_metadata()?;

        // Create managed plugin
        let managed = ManagedPlugin {
            metadata: metadata.clone(),
            instance,
            manifest: discovered.manifest.clone(),
            enabled: true,
        };

        // Store plugin
        self.plugins.write().insert(plugin_id.clone(), managed);
        self.enabled_plugins.write().insert(plugin_id.clone(), true);

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

        if let Some(mut plugin) = plugins.remove(plugin_id) {
            plugin.instance.cleanup()?;
            info!("Plugin unloaded: {}", plugin_id);
            Ok(())
        } else {
            Err(PluginError::NotFound(plugin_id.to_string()))
        }
    }

    /// Enable a plugin (interior mutability safe)
    pub fn enable_plugin(&self, plugin_id: &str) -> Result<()> {
        let plugins = self.plugins.read();

        if plugins.contains_key(plugin_id) {
            self.enabled_plugins
                .write()
                .insert(plugin_id.to_string(), true);
            info!("Plugin enabled: {}", plugin_id);
            Ok(())
        } else {
            Err(PluginError::NotFound(plugin_id.to_string()))
        }
    }

    /// Disable a plugin (interior mutability safe)
    pub fn disable_plugin(&self, plugin_id: &str) -> Result<()> {
        let plugins = self.plugins.read();

        if plugins.contains_key(plugin_id) {
            self.enabled_plugins
                .write()
                .insert(plugin_id.to_string(), false);
            info!("Plugin disabled: {}", plugin_id);
            Ok(())
        } else {
            Err(PluginError::NotFound(plugin_id.to_string()))
        }
    }

    /// Check if a plugin is enabled
    pub fn is_plugin_enabled(&self, plugin_id: &str) -> bool {
        self.enabled_plugins
            .read()
            .get(plugin_id)
            .copied()
            .unwrap_or(false)
    }

    /// Get list of all plugins with their enabled status
    pub fn list_plugins(&self) -> Vec<PluginListItem> {
        let plugins = self.plugins.read();
        let enabled = self.enabled_plugins.read();

        plugins
            .iter()
            .map(|(id, p)| PluginListItem {
                id: id.clone(),
                manifest: p.manifest.clone(),
                enabled: enabled.get(id).copied().unwrap_or(false),
                instance: if p.enabled { Some(()) } else { None },
            })
            .collect()
    }

    /// Get plugin metadata
    pub fn get_plugin_metadata(&self, plugin_id: &str) -> Option<PluginMetadata> {
        self.plugins
            .read()
            .get(plugin_id)
            .map(|p| p.metadata.clone())
    }

    /// Access a plugin instance mutably (e.g. for UI interaction)
    pub fn with_plugin_instance<F, R>(&self, plugin_id: &str, f: F) -> Option<R>
    where
        F: FnOnce(&mut PluginInstance) -> R,
    {
        let mut plugins = self.plugins.write();
        let plugin = plugins.get_mut(plugin_id)?;
        Some(f(&mut plugin.instance))
    }

    /// Dispatch an event to all enabled plugins
    pub fn dispatch_event(&mut self, event: &PluginEvent) -> Vec<PluginResponse> {
        debug!("Dispatching event: {:?}", event);

        let mut responses = Vec::new();
        let plugin_ids: Vec<String> = self.plugins.read().keys().cloned().collect();

        for plugin_id in plugin_ids {
            // Check if plugin is enabled
            if !self.is_plugin_enabled(&plugin_id) {
                continue;
            }

            // Get mutable access to plugin
            let mut plugins = self.plugins.write();
            if let Some(plugin) = plugins.get_mut(&plugin_id) {
                match plugin.instance.on_event(event) {
                    Ok(response) => {
                        debug!("Plugin '{}' responded: {:?}", plugin_id, response);
                        responses.push(response);
                    }
                    Err(e) => {
                        error!("Plugin '{}' error handling event: {}", plugin_id, e);
                        responses.push(PluginResponse::Error {
                            message: e.to_string(),
                        });
                    }
                }
            }
        }

        responses
    }

    /// Dispatch event to a specific plugin
    pub fn dispatch_event_to_plugin(
        &mut self,
        plugin_id: &str,
        event: &PluginEvent,
    ) -> Result<PluginResponse> {
        debug!("Dispatching event to plugin '{}': {:?}", plugin_id, event);

        // Check if plugin is enabled
        if !self.is_plugin_enabled(plugin_id) {
            return Err(PluginError::ExecutionError(format!(
                "Plugin '{}' is disabled",
                plugin_id
            )));
        }

        let mut plugins = self.plugins.write();
        let plugin = plugins
            .get_mut(plugin_id)
            .ok_or_else(|| PluginError::NotFound(plugin_id.to_string()))?;

        plugin.instance.on_event(event)
    }

    /// Get the plugins directory path
    pub fn plugins_dir(&self) -> &std::path::Path {
        self.loader.plugins_dir()
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
        let mut temp_instance = loaded.instantiate(capabilities, 60)?;
        temp_instance.init()?;
        let metadata = temp_instance.get_metadata()?;
        temp_instance.cleanup()?;

        let plugin_id = metadata.id.clone();

        // Check if plugin is already installed
        if self.plugins.read().contains_key(&plugin_id) {
            return Err(PluginError::LoadError(format!(
                "Plugin '{}' is already installed",
                plugin_id
            )));
        }

        // Create plugin directory
        let plugin_dir = self.plugins_dir().join(&plugin_id);
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
            metadata.id, metadata.name, metadata.version, metadata.description, metadata.author
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
            .find(|p| p.manifest.plugin.id == plugin_id)
            .ok_or_else(|| {
                PluginError::LoadError("Failed to discover newly installed plugin".to_string())
            })?;

        // Load the plugin
        self.load_plugin(plugin_info)?;

        info!("Plugin '{}' installed and loaded successfully", plugin_id);
        Ok(plugin_id)
    }

    /// Get aggregated network logs from all enabled plugins
    pub fn get_network_log(&self) -> Vec<(std::time::SystemTime, String)> {
        let mut all_logs = Vec::new();
        let plugins = self.plugins.read();

        for plugin in plugins.values() {
            if plugin.enabled {
                let logs = plugin.instance.get_network_log();
                all_logs.extend(logs);
            }
        }

        // Sort by time
        all_logs.sort_by(|a, b| a.0.cmp(&b.0));
        all_logs
    }
}

/// A managed plugin with its instance and metadata
struct ManagedPlugin {
    metadata: PluginMetadata,
    instance: PluginInstance,
    manifest: crate::types::PluginManifest,
    enabled: bool,
}

#[cfg(test)]
mod tests;
