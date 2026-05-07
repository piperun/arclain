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
            drop(plugins);
            self.invalidate_top_tabs_cache();
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
            drop(plugins);
            self.invalidate_top_tabs_cache();
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

    /// Cheap counts-only summary suitable for status-bar style usage
    /// where the caller only needs `(total, enabled)` and does NOT need
    /// each plugin's manifest. The status bar previously called
    /// `list_plugins()` every render frame and threw away everything
    /// except the counts — cloning every plugin's manifest (Vec of
    /// capability/domain strings) per frame for nothing (audit
    /// finding P5).
    pub fn status_summary(&self) -> super::types::PluginStatusSummary {
        let plugins = self.plugins.read();
        let enabled = self.enabled_plugins.read();
        let total = plugins.len();
        let enabled_count = plugins
            .keys()
            .filter(|id| enabled.get(id.as_str()).copied().unwrap_or(false))
            .count();
        super::types::PluginStatusSummary {
            total,
            enabled: enabled_count,
        }
    }

    /// Get plugin metadata
    pub fn get_plugin_metadata(&self, plugin_id: &str) -> Option<PluginMetadata> {
        self.plugins
            .read()
            .get(plugin_id)
            .map(|p| p.metadata.clone())
    }

    /// Get all top-level tabs registered by enabled plugins
    /// Returns tabs sorted by priority (lower priority = earlier in list).
    ///
    /// Result is cached and re-used until `invalidate_top_tabs_cache` is
    /// called (audit finding P3 — pre-fix this WASM-called every enabled
    /// plugin every render frame). Cache is dropped automatically on
    /// `enable_plugin` / `disable_plugin` / `load_plugin`.
    pub fn get_all_top_tabs(&self) -> Vec<(String, crate::types::TopTabConfig)> {
        if let Some(cached) = self.cached_top_tabs.lock().clone() {
            return cached;
        }

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
        drop(plugins);
        drop(enabled);

        // Sort by priority (lower = first)
        all_tabs.sort_by(|a, b| a.1.priority.cmp(&b.1.priority));
        // Cache the sorted output so subsequent renders just clone from
        // cache without re-sorting or re-querying any plugin.
        *self.cached_top_tabs.lock() = Some(all_tabs.clone());
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

    /// Get a snapshot of all plugin settings for persistence.
    ///
    /// Each plugin carries a `settings_dirty: AtomicBool` that flips to
    /// `true` when [`HostFunctions::impl_set_setting`] writes a value.
    /// We swap it back to `false` here and only re-lock + clone the
    /// instance whose flag was set; everyone else gets returned from
    /// the manager-side `settings_cache` (audit P14). The very first
    /// call always populates the cache because instances start with
    /// `dirty == true`.
    pub fn get_all_settings(&self) -> HashMap<String, HashMap<String, String>> {
        use std::sync::atomic::Ordering;

        let plugins = self.plugins.read();
        let mut cache = self.settings_cache.lock();

        for (id, plugin) in plugins.iter() {
            // AcqRel pairs with the Release store in `impl_set_setting`.
            // If a write lands between the swap and the lock below it
            // re-flips dirty to true, so the next call picks it up.
            let was_dirty = plugin.settings_dirty.swap(false, Ordering::AcqRel);
            if was_dirty {
                let instance = plugin.instance.lock();
                if let Some(settings) = instance.get_settings() {
                    cache.insert(id.clone(), settings);
                }
            }
        }

        // Drop unloaded plugins from the cache so the snapshot doesn't
        // resurrect stale entries after a plugin is unloaded.
        cache.retain(|id, _| plugins.contains_key(id));

        let mut all_settings: HashMap<String, HashMap<String, String>> = cache.clone();

        // Merge with initial settings to preserve settings for plugins
        // that failed to load or aren't active.
        for (id, settings) in &self.initial_settings {
            all_settings.entry(id.clone()).or_insert_with(|| settings.clone());
        }

        all_settings
    }

    /// Get a snapshot of a single plugin's settings.
    ///
    /// Returns the live in-memory settings if the plugin is loaded,
    /// otherwise falls back to the initial_settings provided at
    /// `PluginManager::new`. Used by the detail-view UI event handler
    /// so a single click only fetches the one plugin's settings
    /// rather than `get_all_settings`-ing the whole map (audit P7).
    /// Same dirty-bit + cache short-circuit as `get_all_settings`.
    pub fn get_settings_for(&self, plugin_id: &str) -> Option<HashMap<String, String>> {
        use std::sync::atomic::Ordering;

        let plugins = self.plugins.read();
        if let Some(plugin) = plugins.get(plugin_id) {
            let was_dirty = plugin.settings_dirty.swap(false, Ordering::AcqRel);
            if was_dirty {
                let instance = plugin.instance.lock();
                if let Some(settings) = instance.get_settings() {
                    self.settings_cache
                        .lock()
                        .insert(plugin_id.to_string(), settings.clone());
                    return Some(settings);
                }
            } else if let Some(cached) = self.settings_cache.lock().get(plugin_id) {
                return Some(cached.clone());
            }
        }
        self.initial_settings.get(plugin_id).cloned()
    }
}
