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

pub use types::PluginListItem;
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
    pub(crate) initial_settings: HashMap<String, HashMap<String, String>>,
    /// Channel sender for async event dispatch (non-blocking)
    pub(crate) event_sender: std::sync::mpsc::Sender<PluginEvent>,
    /// Handle to the event worker thread
    _event_worker_handle: Option<std::thread::JoinHandle<()>>,
    /// Reactive signal for metadata updates
    pub(crate) metadata_signal: Option<arclain_signals::Signal<Option<serde_json::Value>>>,
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
            library_service: None,
            content_cache: None,
            resource_manager: None,
            async_http_client: None,
            initial_settings,
            event_sender,
            _event_worker_handle: Some(worker_handle),
            metadata_signal: None,
        })
    }

    /// Set the metadata signal for reactive updates
    pub fn set_metadata_signal(
        &mut self,
        signal: arclain_signals::Signal<Option<serde_json::Value>>,
    ) {
        self.metadata_signal = Some(signal.clone());
        let plugins = self.plugins.read();
        for plugin in plugins.values() {
            let mut instance = plugin.instance.lock();
            instance.set_metadata_signal(Some(signal.clone()));
        }
    }

    /// Update the library service for all plugins
    pub fn set_library_service(&mut self, lib_svc: Arc<arclain_core::LibraryService>) {
        self.library_service = Some(lib_svc.clone());
        let plugins = self.plugins.read();
        for plugin in plugins.values() {
            let mut instance = plugin.instance.lock();
            instance.set_library_service(Some(lib_svc.clone()));
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
    pub fn set_async_http_client(&mut self, client: Arc<arclain_network::AsyncHttpClient>) {
        self.async_http_client = Some(client.clone());
        let plugins = self.plugins.read();
        for (plugin_id, plugin) in plugins.iter() {
            let mut instance = plugin.instance.lock();
            instance.set_async_http_client(Some(client.clone()));

            // Auto-approve network domains from manifest for already-loaded plugins
            for domain in &plugin.manifest.capabilities.network_domains {
                client.approve_domain(plugin_id, domain);
            }
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
}

#[cfg(test)]
mod tests;
