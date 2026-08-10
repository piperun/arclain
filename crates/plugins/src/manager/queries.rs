//! Query methods for plugin manager

use super::types::PluginListItem;
use super::{EnabledPluginSnapshot, PluginManager};
use crate::runtime::PluginInstance;
use crate::types::{PluginError, PluginIdentityKey, PluginMetadata, Result};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

impl PluginManager {
    /// Enable a plugin (interior mutability safe)
    pub fn enable_plugin(&self, plugin_id: &str) -> Result<()> {
        let identity_key = PluginIdentityKey::parse(plugin_id)
            .map_err(|_| PluginError::NotFound(plugin_id.to_string()))?;
        let plugins = self.plugins.read();

        if plugins.contains_key(&identity_key) {
            self.enabled_plugins.write().insert(identity_key, true);
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
        let identity_key = PluginIdentityKey::parse(plugin_id)
            .map_err(|_| PluginError::NotFound(plugin_id.to_string()))?;
        let plugins = self.plugins.read();

        if plugins.contains_key(&identity_key) {
            self.enabled_plugins.write().insert(identity_key, false);
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
        let Ok(identity_key) = PluginIdentityKey::parse(plugin_id) else {
            return false;
        };
        self.enabled_plugins
            .read()
            .get(&identity_key)
            .copied()
            .unwrap_or(false)
    }

    /// Get list of all plugins with their enabled status
    pub fn list_plugins(&self) -> Vec<PluginListItem> {
        let plugins = self.plugins.read();
        let enabled = self.enabled_plugins.read();

        plugins
            .iter()
            .map(|(identity_key, p)| PluginListItem {
                id: p.metadata.id.clone(),
                manifest: p.manifest.clone(),
                enabled: enabled.get(identity_key).copied().unwrap_or(false),
                instance: if p.enabled { Some(()) } else { None },
            })
            .collect()
    }

    /// Every plugin discovered on disk that failed to load during
    /// [`PluginManager::init`], most-recent first.
    /// [`PluginManager::install_plugin_package`] and
    /// [`PluginManager::reload_plugin`] do *not* feed this list: both return
    /// their failure directly to the caller (via `Result`) instead, so a
    /// rejected install/reload is only ever visible to whoever made that
    /// specific call, never recorded here.
    /// Used by the application facade to report a plugin's `load_error`
    /// alongside the plugins that *did* load successfully -- see
    /// `super::types::FailedPlugin`'s own doc comment.
    pub fn failed_plugins(&self) -> Vec<super::types::FailedPlugin> {
        self.failed_plugins.lock().iter().rev().cloned().collect()
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
            .filter(|id| enabled.get(*id).copied().unwrap_or(false))
            .count();
        super::types::PluginStatusSummary {
            total,
            enabled: enabled_count,
        }
    }

    /// Get plugin metadata
    pub fn get_plugin_metadata(&self, plugin_id: &str) -> Option<PluginMetadata> {
        let identity_key = PluginIdentityKey::parse(plugin_id).ok()?;
        self.plugins
            .read()
            .get(&identity_key)
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

        let all_tabs = self.enabled_plugin_snapshot().get_all_top_tabs();
        // Cache the sorted output so subsequent renders just clone from
        // cache without re-sorting or re-querying any plugin.
        *self.cached_top_tabs.lock() = Some(all_tabs.clone());
        all_tabs
    }

    /// Clone enabled plugin IDs and instance handles into a detached value.
    ///
    /// Callers that wrap `PluginManager` in an outer mutex should construct
    /// this snapshot while holding that mutex, then drop the guard before
    /// calling plugin instance methods on the returned value.
    pub fn enabled_plugin_snapshot(&self) -> EnabledPluginSnapshot {
        let plugins = self.plugins.read();
        let enabled = self.enabled_plugins.read();
        let snapshot = plugins
            .iter()
            .filter(|(identity_key, _)| enabled.get(*identity_key).copied().unwrap_or(false))
            .map(|(identity_key, plugin)| {
                (
                    identity_key.clone(),
                    plugin.metadata.id.clone(),
                    plugin.instance.clone(),
                )
            })
            .collect();

        EnabledPluginSnapshot::new(snapshot)
    }

    /// Get a thread-safe handle to a plugin instance.
    /// Returns None if the plugin is not found.
    /// This allows the caller to manage locking strategies (blocking vs try_lock).
    pub fn get_plugin_instance(&self, plugin_id: &str) -> Option<Arc<Mutex<PluginInstance>>> {
        let identity_key = PluginIdentityKey::parse(plugin_id).ok()?;
        self.plugins
            .read()
            .get(&identity_key)
            .map(|p| p.instance.clone())
    }

    /// Check a loaded plugin instance's immutable manifest capabilities.
    pub fn plugin_has_capabilities(
        &self,
        plugin_id: &str,
        required: &[crate::types::PluginCapability],
    ) -> bool {
        let Some(instance) = self.get_plugin_instance(plugin_id) else {
            return false;
        };
        let authorized = instance.lock().has_capabilities(required);
        authorized
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

    /// Non-blocking variant of `with_plugin_instance`. Returns:
    /// - `Some(Some(value))` — the lock was free, `f` ran and produced `value`.
    /// - `Some(None)` — the plugin id exists but the instance lock is held by
    ///   another thread (e.g. a background fetch holding it during HTTP).
    ///   Callers on the UI thread should fall back to a cached value or
    ///   render an empty state for this frame; do NOT block.
    /// - `None` — the plugin id is unknown.
    ///
    /// Use this on the UI thread for plugin reads (`get_ui_layout`,
    /// `get_top_tabs`, etc.) so a long-running plugin event on a worker
    /// thread doesn't freeze the UI.
    pub fn try_with_plugin_instance<F, R>(&self, plugin_id: &str, f: F) -> Option<Option<R>>
    where
        F: FnOnce(&mut PluginInstance) -> R,
        R: 'static,
    {
        let instance_arc = self.get_plugin_instance(plugin_id)?;
        let result = if let Some(mut instance) = instance_arc.try_lock() {
            Some(f(&mut instance))
        } else {
            None
        };
        Some(result)
    }

    /// Get the plugins directory path
    pub fn plugins_dir(&self) -> &std::path::Path {
        self.loader.plugins_dir()
    }

    /// Get aggregated network logs from all enabled plugins
    pub fn get_network_log(&self) -> Vec<(std::time::SystemTime, String)> {
        self.enabled_plugin_snapshot().get_network_log()
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

        let mut all_settings = HashMap::with_capacity(cache.len() + self.initial_settings.len());
        for (identity_key, settings) in cache.iter() {
            if let Some(plugin) = plugins.get(identity_key) {
                all_settings.insert(plugin.metadata.id.clone(), settings.clone());
            }
        }

        // Merge with initial settings to preserve settings for plugins
        // that failed to load or aren't active.
        for (identity_key, initial) in &self.initial_settings {
            let output_id = plugins
                .get(identity_key)
                .map(|plugin| plugin.metadata.id.clone())
                .unwrap_or_else(|| initial.original_id.clone());
            all_settings
                .entry(output_id)
                .or_insert_with(|| initial.values.clone());
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

        let identity_key = PluginIdentityKey::parse(plugin_id).ok()?;
        let plugins = self.plugins.read();
        if let Some(plugin) = plugins.get(&identity_key) {
            let was_dirty = plugin.settings_dirty.swap(false, Ordering::AcqRel);
            if was_dirty {
                let instance = plugin.instance.lock();
                if let Some(settings) = instance.get_settings() {
                    self.settings_cache
                        .lock()
                        .insert(identity_key.clone(), settings.clone());
                    return Some(settings);
                }
            } else if let Some(cached) = self.settings_cache.lock().get(&identity_key) {
                return Some(cached.clone());
            }
        }
        self.initial_settings
            .get(&identity_key)
            .map(|entry| entry.values.clone())
    }
}
