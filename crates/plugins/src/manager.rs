//! Plugin manager for lifecycle and event dispatching

use crate::loader::{DiscoveredPlugin, PluginLoader};
use crate::runtime::PluginInstance;
use crate::types::{PluginError, PluginEvent, PluginMetadata, PluginResponse, Result};
use arclain_core::ArchiveBackend;
use parking_lot::{Mutex, RwLock};
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
    backend: Option<Arc<dyn ArchiveBackend>>,
    metadata_store: Option<Arc<arclain_db::MetadataStore>>,
    content_cache: Option<Arc<arclain_data::ContentCache>>,
    resource_manager: Option<Arc<arclain_data::ResourceManager>>,
    async_http_client: Option<Arc<arclain_http::AsyncHttpClient>>,
    initial_settings: HashMap<String, HashMap<String, String>>,
    /// Channel sender for async event dispatch (non-blocking)
    event_sender: std::sync::mpsc::Sender<PluginEvent>,
    /// Handle to the event worker thread
    _event_worker_handle: Option<std::thread::JoinHandle<()>>,
    /// Reactive signal for metadata updates
    metadata_signal: Option<arclain_signals::Signal<Option<serde_json::Value>>>,
}

impl PluginManager {
    /// Create a new plugin manager
    pub fn new(
        plugins_dir: PathBuf,
        initial_settings: HashMap<String, HashMap<String, String>>,
    ) -> Result<Self> {
        let loader = PluginLoader::new(plugins_dir)?;
        let plugins = Arc::new(RwLock::new(HashMap::new()));
        let enabled_plugins = Arc::new(RwLock::new(HashMap::new()));

        // Create channel for async event dispatch
        let (event_sender, event_receiver) = std::sync::mpsc::channel::<PluginEvent>();

        // Spawn worker thread to process events
        let plugins_clone = plugins.clone();
        let enabled_plugins_clone = enabled_plugins.clone();
        let worker_handle = std::thread::spawn(move || {
            Self::event_worker(event_receiver, plugins_clone, enabled_plugins_clone);
        });

        Ok(Self {
            loader,
            plugins,
            enabled_plugins,
            backend: None,
            metadata_store: None,
            content_cache: None,
            resource_manager: None,
            async_http_client: None,
            initial_settings,
            event_sender,
            _event_worker_handle: Some(worker_handle),
            metadata_signal: None,
        })
    }

    /// Create a new plugin manager with archive backend
    pub fn with_backend(
        plugins_dir: PathBuf,
        backend: Arc<dyn ArchiveBackend>,
        initial_settings: HashMap<String, HashMap<String, String>>,
    ) -> Result<Self> {
        let loader = PluginLoader::new(plugins_dir)?;
        let plugins = Arc::new(RwLock::new(HashMap::new()));
        let enabled_plugins = Arc::new(RwLock::new(HashMap::new()));

        // Create channel for async event dispatch
        let (event_sender, event_receiver) = std::sync::mpsc::channel::<PluginEvent>();

        // Spawn worker thread to process events
        let plugins_clone = plugins.clone();
        let enabled_plugins_clone = enabled_plugins.clone();
        let worker_handle = std::thread::spawn(move || {
            Self::event_worker(event_receiver, plugins_clone, enabled_plugins_clone);
        });

        Ok(Self {
            loader,
            plugins,
            enabled_plugins,
            backend: Some(backend),
            metadata_store: None,
            content_cache: None,
            resource_manager: None,
            async_http_client: None,
            initial_settings,
            event_sender,
            _event_worker_handle: Some(worker_handle),
            metadata_signal: None,
        })
    }

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

    /// Set the metadata signal for reactive updates
    pub fn set_metadata_signal(
        &mut self,
        signal: arclain_signals::Signal<Option<serde_json::Value>>,
    ) {
        self.metadata_signal = Some(signal);
    }

    /// Update the metadata cache for all plugins
    pub fn set_metadata_store(&mut self, store: Arc<arclain_db::MetadataStore>) {
        self.metadata_store = Some(store.clone());
        let plugins = self.plugins.read();
        for plugin in plugins.values() {
            let mut instance = plugin.instance.lock();
            instance.set_metadata_store(Some(store.clone()));
        }
    }

    /// Set content cache
    pub fn set_content_cache(&mut self, cache: Arc<arclain_data::ContentCache>) {
        self.content_cache = Some(cache.clone());
        let plugins = self.plugins.read();
        for plugin in plugins.values() {
            let mut instance = plugin.instance.lock();
            instance.set_content_cache(Some(cache.clone()));
        }
    }

    /// Set cache database for new ProductMetadata table
    pub fn set_cache_db(&mut self, db: Arc<arclain_db::SqliteDb>) {
        let plugins = self.plugins.read();
        for plugin in plugins.values() {
            let mut instance = plugin.instance.lock();
            instance.set_cache_db(Some(db.clone()));
        }
    }

    /// Set resource manager
    pub fn set_resource_manager(&mut self, manager: Arc<arclain_data::ResourceManager>) {
        self.resource_manager = Some(manager.clone());
        let plugins = self.plugins.read();
        for plugin in plugins.values() {
            let mut instance = plugin.instance.lock();
            instance.set_resource_manager(Some(manager.clone()));
        }
    }

    /// Set async http client
    pub fn set_async_http_client(&mut self, client: Arc<arclain_http::AsyncHttpClient>) {
        self.async_http_client = Some(client.clone());
        let plugins = self.plugins.read();
        for plugin in plugins.values() {
            let mut instance = plugin.instance.lock();
            instance.set_async_http_client(Some(client.clone()));
        }
    }

    /// Set the archive context for all plugins
    pub fn set_archive_context(&mut self, archive_path: Option<String>, password: Option<String>) {
        let plugins = self.plugins.read();
        for plugin in plugins.values() {
            let mut instance = plugin.instance.lock();
            instance.set_archive_context(archive_path.clone(), password.clone());
        }
    }

    // ... (lines 58-115 unchanged)

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

        // Get initial settings for this plugin
        let settings = self
            .initial_settings
            .get(&plugin_id)
            .cloned()
            .unwrap_or_default();

        // Instantiate the plugin with backend if available
        let mut instance = if let Some(ref backend) = self.backend {
            loaded.instantiate_with_backend(
                capabilities.clone(),
                rate_limit,
                Some(backend.clone()),
                self.metadata_store.clone(),
                settings,
                self.metadata_signal.clone(),
            )?
        } else {
            loaded.instantiate_with_backend(
                capabilities.clone(),
                rate_limit,
                None,
                self.metadata_store.clone(),
                settings,
                self.metadata_signal.clone(),
            )?
        };

        // Initialize the plugin
        instance.init()?;

        // Get metadata
        let metadata = instance.get_metadata()?;

        // Create managed plugin
        let managed = ManagedPlugin {
            metadata: metadata.clone(),
            instance: Arc::new(Mutex::new(instance)),
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

        if let Some(plugin) = plugins.remove(plugin_id) {
            plugin.instance.lock().cleanup()?;
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

    /// Get all top-level tabs registered by enabled plugins
    /// Returns tabs sorted by priority (lower priority = earlier in list)
    pub fn get_all_top_tabs(&self) -> Vec<(String, crate::types::TopTabConfig)> {
        let mut all_tabs = Vec::new();
        let plugins = self.plugins.read();
        let enabled = self.enabled_plugins.read();

        for (plugin_id, plugin) in plugins.iter() {
            // Only get tabs from enabled plugins
            if !enabled.get(plugin_id).copied().unwrap_or(false) {
                continue;
            }

            // Try to get tabs from the plugin
            if let Some(mut instance) = plugin.instance.try_lock() {
                match instance.get_top_tabs() {
                    Ok(tabs) => {
                        for tab in tabs {
                            all_tabs.push((plugin_id.clone(), tab));
                        }
                    }
                    Err(e) => {
                        debug!("Failed to get top tabs from {}: {:?}", plugin_id, e);
                    }
                }
            }
        }

        // Sort by priority (lower = first)
        all_tabs.sort_by(|a, b| a.1.priority.cmp(&b.1.priority));
        all_tabs
    }

    /// Get a thread-safe handle to a plugin instance.
    /// Returns None if the plugin is not found.
    /// This allows the caller to manage locking strategies (blocking vs try_lock).
    pub fn get_plugin_instance(&self, plugin_id: &str) -> Option<Arc<Mutex<PluginInstance>>> {
        self.plugins
            .read()
            .get(plugin_id)
            .map(|p| p.instance.clone())
    }

    /// Access a plugin instance mutably (e.g. for UI interaction)
    /// This now acquires a granular lock on the specific plugin instance.
    pub fn with_plugin_instance<F, R>(&self, plugin_id: &str, f: F) -> Option<R>
    where
        F: FnOnce(&mut PluginInstance) -> R,
        R: 'static,
    {
        // Use helper to get owned Arc, ensuring map lock is dropped
        let instance_arc = self.get_plugin_instance(plugin_id)?;

        // Granular lock on instance
        let mut instance = instance_arc.lock();
        Some(f(&mut instance))
    }

    /// Send a UI event to a plugin asynchronously (non-blocking).
    /// The callback will be called on the background thread with the plugin's response.
    /// This prevents the UI from freezing during plugin execution.
    pub fn send_event_async<F>(
        &self,
        plugin_id: &str,
        event_id: &str,
        value: Option<String>,
        callback: F,
    ) where
        F: FnOnce(std::result::Result<Vec<crate::types::PluginUiElement>, String>) + Send + 'static,
    {
        // Get the plugin instance Arc before spawning thread
        let Some(instance_arc) = self.get_plugin_instance(plugin_id) else {
            callback(Err(format!("Plugin '{}' not found", plugin_id)));
            return;
        };

        let event_id = event_id.to_string();

        std::thread::spawn(move || {
            // Lock the instance on the background thread
            let mut instance = instance_arc.lock();

            match instance.send_ui_event(&event_id, value) {
                Ok(actions) => {
                    // Convert PluginAction to PluginUiElement for the callback
                    // For now, just pass an empty vec since actions are handled differently
                    callback(Ok(vec![]));

                    // Actions would need to be processed here or passed to a channel
                    // For UI refresh purposes, we'll handle this differently
                    if !actions.is_empty() {
                        tracing::debug!("Plugin returned {} actions (async)", actions.len());
                    }
                }
                Err(e) => {
                    callback(Err(format!("Plugin error: {:?}", e)));
                }
            }
        });
    }

    /// Dispatch an event to all enabled plugins asynchronously
    pub fn dispatch_event_async(&self, event: PluginEvent) {
        let plugins = self.plugins.clone();
        let enabled_plugins = self.enabled_plugins.clone();

        std::thread::spawn(move || {
            debug!("Async dispatching event: {:?}", event);
            let plugin_ids: Vec<String> = plugins.read().keys().cloned().collect();

            for plugin_id in plugin_ids {
                // Check if plugin is enabled
                let is_enabled = enabled_plugins
                    .read()
                    .get(&plugin_id)
                    .copied()
                    .unwrap_or(false);

                if !is_enabled {
                    continue;
                }

                // Get instance handle
                let instance_arc = {
                    let map = plugins.read();
                    if let Some(p) = map.get(&plugin_id) {
                        p.instance.clone()
                    } else {
                        continue;
                    }
                };

                // Call plugin
                let mut instance = instance_arc.lock();

                // Map PluginEvent to UI event for compatibility
                // (Since on_event is not exposed in WIT yet)
                if let PluginEvent::OnArchiveOpen { path, .. } = &event {
                    let id = "event:archive_opened".to_string();
                    let value = Some(path.clone());

                    if let Err(e) = instance.send_ui_event(&id, value) {
                        error!("Async event error for {}: {}", plugin_id, e);
                    }
                }
            }
        });
    }

    /// Background worker that processes events from the channel.
    /// Runs on a dedicated thread and never blocks the caller.
    fn event_worker(
        receiver: std::sync::mpsc::Receiver<PluginEvent>,
        plugins: Arc<RwLock<HashMap<String, ManagedPlugin>>>,
        enabled_plugins: Arc<RwLock<HashMap<String, bool>>>,
    ) {
        info!("Plugin event worker started");

        while let Ok(event) = receiver.recv() {
            debug!("Event worker processing: {:?}", event);

            let plugin_ids: Vec<String> = plugins.read().keys().cloned().collect();

            for plugin_id in plugin_ids {
                // Check if plugin is enabled
                let is_enabled = enabled_plugins
                    .read()
                    .get(&plugin_id)
                    .copied()
                    .unwrap_or(false);

                if !is_enabled {
                    continue;
                }

                // Get instance handle
                let instance_arc = {
                    let map = plugins.read();
                    if let Some(p) = map.get(&plugin_id) {
                        p.instance.clone()
                    } else {
                        continue;
                    }
                };

                // Call plugin
                let mut instance = instance_arc.lock();

                // Match event to set context and dispatch
                match &event {
                    PluginEvent::OnArchiveOpen { path, password, .. } => {
                        // Set context first (on background thread!)
                        instance.set_archive_context(Some(path.clone()), password.clone());

                        // Then dispatch event
                        let id = "event:archive_opened".to_string();
                        let value = Some(path.clone());

                        if let Err(e) = instance.send_ui_event(&id, value) {
                            error!("Event worker error for {}: {:?}", plugin_id, e);
                        }
                    }
                    _ => {
                        // Other events (future)
                    }
                }
            }
        }

        info!("Plugin event worker stopped");
    }

    /// Send an event to all enabled plugins asynchronously.
    /// This method returns immediately - never blocks the caller.
    /// Events are processed by a background worker thread.
    pub fn send_event(&self, event: PluginEvent) {
        if let Err(e) = self.event_sender.send(event) {
            error!("Failed to send event to worker: {}", e);
        }
    }

    /// Get a cloned sender for lock-free event dispatch.
    /// Use this to avoid needing to lock the PluginManager when sending events.
    pub fn get_event_sender(&self) -> std::sync::mpsc::Sender<PluginEvent> {
        self.event_sender.clone()
    }

    /// Dispatch an event to all enabled plugins
    pub fn dispatch_event(&mut self, event: &PluginEvent) -> Vec<PluginResponse> {
        debug!("Dispatching event: {:?}", event);

        let mut responses = Vec::new();
        // Only need read lock now since instances are internally locked
        let plugin_ids: Vec<String> = self.plugins.read().keys().cloned().collect();

        for plugin_id in plugin_ids {
            // Check if plugin is enabled
            if !self.is_plugin_enabled(&plugin_id) {
                continue;
            }

            // Get read access to plugins map and clone Arc
            let instance_arc = {
                let plugins = self.plugins.read();
                if let Some(plugin) = plugins.get(&plugin_id) {
                    plugin.instance.clone()
                } else {
                    continue;
                }
            };

            // Acquire instance lock
            let mut instance = instance_arc.lock();
            match instance.on_event(event) {
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

        let instance_arc = {
            let plugins = self.plugins.read();
            plugins
                .get(plugin_id)
                .ok_or_else(|| PluginError::NotFound(plugin_id.to_string()))?
                .instance
                .clone()
        };

        // Acquire instance lock
        let mut instance = instance_arc.lock();
        instance.on_event(event)
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
        let mut temp_instance = loaded.instantiate(capabilities, 60, HashMap::new())?;
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
        // Use read lock
        let plugins = self.plugins.read();

        for plugin in plugins.values() {
            if plugin.enabled {
                // Acquire instance lock
                let instance = plugin.instance.lock();
                let logs = instance.get_network_log();
                all_logs.extend(logs);
            }
        }

        // Sort by time
        all_logs.sort_by(|a, b| a.0.cmp(&b.0));
        all_logs
    }

    /// Get a snapshot of all plugin settings for persistence
    pub fn get_all_settings(&self) -> HashMap<String, HashMap<String, String>> {
        let plugins = self.plugins.read();
        let mut all_settings = HashMap::new();

        for (id, plugin) in plugins.iter() {
            let instance = plugin.instance.lock();
            // We need to access the settings from the store data
            // Since PluginInstance wraps the store, we need to modify PluginInstance/runtime
            // to expose a way to get settings.
            if let Some(settings) = (*instance).get_settings() {
                all_settings.insert(id.clone(), settings);
            }
        }

        // Merge with initial settings to preserve settings for plugins that failed to load or aren't active
        for (id, settings) in &self.initial_settings {
            if !all_settings.contains_key(id) {
                all_settings.insert(id.clone(), settings.clone());
            }
        }

        all_settings
    }
}

/// A managed plugin with its instance and metadata
struct ManagedPlugin {
    metadata: PluginMetadata,
    instance: Arc<Mutex<PluginInstance>>,
    manifest: crate::types::PluginManifest,
    enabled: bool,
}

#[cfg(test)]
mod tests;
