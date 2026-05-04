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
mod types;

pub use types::{PluginListItem, PluginStatusSummary};
use types::ManagedPlugin;

use crate::loader::PluginLoader;
use crate::types::{PluginEvent, Result};
use arclain_core::ArchiveBackend;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Manages all loaded plugins and dispatches events
pub struct PluginManager {
    pub(crate) loader: PluginLoader,
    pub(crate) plugins: Arc<RwLock<HashMap<String, ManagedPlugin>>>,
    pub(crate) enabled_plugins: Arc<RwLock<HashMap<String, bool>>>,
    pub(crate) backend: Option<Arc<dyn ArchiveBackend>>,
    pub(crate) library_service: Option<Arc<arclain_core::LibraryService>>,
    pub(crate) content_cache: Option<Arc<arclain_data::ContentCache>>,
    pub(crate) resource_manager: Option<Arc<arclain_data::ResourceManager>>,
    pub(crate) async_http_client: Option<Arc<arclain_network::AsyncHttpClient>>,
    pub(crate) gameta_client:
        Option<Arc<arclain_network::features::gameta_client::GametaClient>>,
    pub(crate) initial_settings: HashMap<String, HashMap<String, String>>,
    /// Channel sender for async event dispatch (non-blocking)
    pub(crate) event_sender: std::sync::mpsc::Sender<PluginEvent>,
    /// Handle to the event worker thread
    _event_worker_handle: Option<std::thread::JoinHandle<()>>,
    /// Reactive signal for metadata updates
    pub(crate) metadata_signal: Option<arclain_signals::Signal<Option<serde_json::Value>>>,
    /// Cached result of `get_all_top_tabs` — invalidated whenever a
    /// plugin is loaded, enabled, or disabled. Avoids the per-frame
    /// WASM call into every enabled plugin (audit finding P3).
    pub(crate) cached_top_tabs:
        parking_lot::Mutex<Option<Vec<(String, crate::types::TopTabConfig)>>>,
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
            library_service: None,
            content_cache: None,
            resource_manager: None,
            async_http_client: None,
            gameta_client: None,
            initial_settings,
            event_sender,
            _event_worker_handle: Some(worker_handle),
            metadata_signal: None,
            cached_top_tabs: parking_lot::Mutex::new(None),
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
            library_service: None,
            content_cache: None,
            resource_manager: None,
            async_http_client: None,
            gameta_client: None,
            initial_settings,
            event_sender,
            _event_worker_handle: Some(worker_handle),
            metadata_signal: None,
            cached_top_tabs: parking_lot::Mutex::new(None),
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
    fn instance_snapshot(
        &self,
    ) -> Vec<Arc<parking_lot::Mutex<crate::runtime::PluginInstance>>> {
        let plugins = self.plugins.read();
        plugins.values().map(|p| p.instance.clone()).collect()
    }

    /// Set the metadata signal for reactive updates
    pub fn set_metadata_signal(
        &mut self,
        signal: arclain_signals::Signal<Option<serde_json::Value>>,
    ) {
        self.metadata_signal = Some(signal.clone());
        for instance in self.instance_snapshot() {
            instance.lock().set_metadata_signal(Some(signal.clone()));
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

    /// Set async http client. Snapshots `(id, instance, network_domains)`
    /// under `plugins.read()`, drops the read guard, then takes per-plugin
    /// `instance.lock()` and `client.approve_domain()` (which acquires
    /// `whitelist.write()`). Without the snapshot/drop, plugins.read +
    /// whitelist.write nest, forming a deadlock cycle (see
    /// `tests/c1_cascading_lock_test.rs`).
    pub fn set_async_http_client(&mut self, client: Arc<arclain_network::AsyncHttpClient>) {
        self.async_http_client = Some(client.clone());

        let snapshot: Vec<(
            String,
            Arc<parking_lot::Mutex<crate::runtime::PluginInstance>>,
            Vec<String>,
        )> = {
            let plugins = self.plugins.read();
            plugins
                .iter()
                .map(|(id, p)| {
                    (
                        id.clone(),
                        p.instance.clone(),
                        p.manifest.capabilities.network_domains.clone(),
                    )
                })
                .collect()
        };

        for (plugin_id, instance, domains) in snapshot {
            instance.lock().set_async_http_client(Some(client.clone()));
            for domain in &domains {
                client.approve_domain(&plugin_id, domain);
            }
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

    /// Set the archive context for all plugins
    pub fn set_archive_context(&mut self, archive_path: Option<String>, password: Option<String>) {
        for instance in self.instance_snapshot() {
            instance
                .lock()
                .set_archive_context(archive_path.clone(), password.clone());
        }
    }
}

#[cfg(test)]
mod tests;
