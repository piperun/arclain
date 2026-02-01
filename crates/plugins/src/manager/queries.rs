//! Query methods for plugin manager

use super::types::PluginListItem;
use super::PluginManager;
use crate::runtime::PluginInstance;
use crate::types::{PluginError, PluginMetadata, Result};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

impl PluginManager {
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

    /// Get the plugins directory path
    pub fn plugins_dir(&self) -> &std::path::Path {
        self.loader.plugins_dir()
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
