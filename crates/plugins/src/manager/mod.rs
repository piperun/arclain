//! Plugin manager for lifecycle and event dispatching
//!
//! ## Module Structure
//! - `types` - PluginListItem, ManagedPlugin types
//! - `lifecycle` - Plugin loading, unloading, installation
//! - `dispatch` - Event dispatching to plugins
//! - `queries` - Query methods for plugin state

mod dispatch;
mod lifecycle;
mod queries;
mod snapshot;
mod types;

pub use snapshot::EnabledPluginSnapshot;
use types::ManagedPlugin;
pub use types::{PluginListItem, PluginStatusSummary};

use crate::loader::PluginLoader;
use crate::types::{PluginEvent, Result};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Manages all loaded plugins and dispatches events
pub struct PluginManager {
    pub(crate) loader: PluginLoader,
    pub(crate) plugins: Arc<RwLock<HashMap<String, ManagedPlugin>>>,
    pub(crate) enabled_plugins: Arc<RwLock<HashMap<String, bool>>>,
    pub(crate) library_service: Option<Arc<arclain_core::LibraryService>>,
    pub(crate) content_cache: Option<Arc<arclain_data::ContentCache>>,
    pub(crate) resource_manager: Option<Arc<arclain_data::ResourceManager>>,
    pub(crate) async_http_client: Option<Arc<arclain_network::AsyncHttpClient>>,
    pub(crate) gameta_client: Option<Arc<arclain_network::features::gameta_client::GametaClient>>,
    pub(crate) initial_settings: HashMap<String, HashMap<String, String>>,
    pub(crate) plugin_log_dir: PathBuf,
    /// Channel sender for async event dispatch (non-blocking)
    pub(crate) event_sender: std::sync::mpsc::Sender<PluginEvent>,
    /// Handle to the event worker thread
    _event_worker_handle: Option<std::thread::JoinHandle<()>>,
    /// Bridge to the host's per-tab signal tree. Plugins read the
    /// currently active archive + password and write metadata to
    /// the active tab's signal through this. See
    /// `crate::active_tab` for the bridge rationale.
    pub(crate) active_tab_bridge: Option<Arc<dyn crate::ActiveTabBridge>>,
    /// Cached result of `get_all_top_tabs` — invalidated whenever a
    /// plugin is loaded, enabled, or disabled. Avoids the per-frame
    /// WASM call into every enabled plugin (audit finding P3).
    pub(crate) cached_top_tabs:
        parking_lot::Mutex<Option<Vec<(String, crate::types::TopTabConfig)>>>,
    /// Cached settings snapshots indexed by plugin id. `get_all_settings`
    /// uses this to avoid locking + cloning instances whose
    /// `settings_dirty` flag is still `false` (audit P14). Populated on
    /// first read; refreshed only for the plugins that flipped dirty
    /// since.
    pub(crate) settings_cache: parking_lot::Mutex<HashMap<String, HashMap<String, String>>>,
}

impl PluginManager {
    /// Create a new plugin manager
    pub fn new(
        plugins_dir: PathBuf,
        initial_settings: HashMap<String, HashMap<String, String>>,
    ) -> Result<Self> {
        Self::new_with_plugin_log_dir(
            plugins_dir,
            initial_settings,
            arclain_core::utilities::plugin_log_dir(),
        )
    }

    pub(crate) fn new_with_plugin_log_dir(
        plugins_dir: PathBuf,
        initial_settings: HashMap<String, HashMap<String, String>>,
        plugin_log_dir: PathBuf,
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
            library_service: None,
            content_cache: None,
            resource_manager: None,
            async_http_client: None,
            gameta_client: None,
            initial_settings,
            plugin_log_dir,
            event_sender,
            _event_worker_handle: Some(worker_handle),
            active_tab_bridge: None,
            cached_top_tabs: parking_lot::Mutex::new(None),
            settings_cache: parking_lot::Mutex::new(HashMap::new()),
        })
    }

    /// Drop the cached top-tabs list. Called from any place that
    /// changes which plugins are enabled or which instances exist.
    pub(crate) fn invalidate_top_tabs_cache(&self) {
        *self.cached_top_tabs.lock() = None;
    }

    /// Snapshot the per-plugin instance Arcs under a brief
    /// `plugins.read()`, then drop the read guard. Every setter below
    /// uses this so `instance.lock()` and downstream mutations
    /// (`whitelist.write()` in particular) never run while the plugins
    /// RwLock is held — which would otherwise create a deadlock cycle
    /// with any concurrent path that holds `whitelist.read()` and
    /// wants `plugins.read()` (parking_lot writer-preference for new
    /// readers).
    fn instance_snapshot(&self) -> Vec<Arc<parking_lot::Mutex<crate::runtime::PluginInstance>>> {
        let plugins = self.plugins.read();
        plugins.values().map(|p| p.instance.clone()).collect()
    }

    /// Install the bridge to the host's per-tab signal tree. Replaces
    /// the pre-bridge `set_metadata_signal` — see `crate::active_tab`
    /// for why this is a bridge instead of a held handle.
    pub fn set_active_tab_bridge(&mut self, bridge: Arc<dyn crate::ActiveTabBridge>) {
        self.active_tab_bridge = Some(bridge.clone());
        for instance in self.instance_snapshot() {
            instance.lock().set_active_tab_bridge(bridge.clone());
        }
    }

    /// Update the library service for all plugins
    pub fn set_library_service(&mut self, lib_svc: Arc<arclain_core::LibraryService>) {
        self.library_service = Some(lib_svc.clone());
        for instance in self.instance_snapshot() {
            instance.lock().set_library_service(Some(lib_svc.clone()));
        }
    }

    /// Set content cache
    pub fn set_content_cache(&mut self, cache: Arc<arclain_data::ContentCache>) {
        self.content_cache = Some(cache.clone());
        for instance in self.instance_snapshot() {
            instance.lock().set_content_cache(Some(cache.clone()));
        }
    }

    /// Set resource manager
    pub fn set_resource_manager(&mut self, manager: Arc<arclain_data::ResourceManager>) {
        self.resource_manager = Some(manager.clone());
        for instance in self.instance_snapshot() {
            instance.lock().set_resource_manager(Some(manager.clone()));
        }
    }

    /// Set async http client. Snapshots manifest-derived network state
    /// under `plugins.read()`, drops the read guard, then takes per-plugin
    /// `instance.lock()` and updates the client's whitelist. Without the
    /// snapshot/drop, plugins.read +
    /// whitelist.write nest, forming a deadlock cycle (see
    /// `tests/c1_cascading_lock_test.rs`).
    pub fn set_async_http_client(&mut self, client: Arc<arclain_network::AsyncHttpClient>) {
        self.async_http_client = Some(client.clone());

        let snapshot: Vec<(
            String,
            Arc<parking_lot::Mutex<crate::runtime::PluginInstance>>,
            Vec<String>,
            bool,
            u32,
        )> = {
            let plugins = self.plugins.read();
            plugins
                .iter()
                .map(|(id, p)| {
                    (
                        id.clone(),
                        p.instance.clone(),
                        p.manifest.capabilities.network_domains.clone(),
                        p.manifest.capabilities.network,
                        p.manifest.rate_limits.http_requests_per_minute,
                    )
                })
                .collect()
        };

        for (plugin_id, instance, domains, network_enabled, requests_per_minute) in snapshot {
            client.configure_plugin(
                &plugin_id,
                arclain_network::PluginNetworkPolicy {
                    network_enabled,
                    requests_per_minute,
                },
            );
            client.replace_plugin_manifest_domains(&plugin_id, &domains);
            instance.lock().set_async_http_client(Some(client.clone()));
        }
    }

    /// Set the gameta server client for all plugins
    pub fn set_gameta_client(
        &mut self,
        client: Arc<arclain_network::features::gameta_client::GametaClient>,
    ) {
        self.gameta_client = Some(client.clone());
        for instance in self.instance_snapshot() {
            instance.lock().set_gameta_client(Some(client.clone()));
        }
    }
}

#[cfg(test)]
mod tests;
