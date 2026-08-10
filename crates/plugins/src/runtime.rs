//! Arclain compatibility adapters over Wirt's product-neutral runtime.

use crate::host_functions::HostFunctions;
use crate::types::{PluginCapability, PluginExtensionPoint, PluginMetadata, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// WASM runtime for executing plugins.
pub struct WasmRuntime(wirt::WasmRuntime);

impl WasmRuntime {
    /// Create a new WASM runtime.
    pub fn new() -> Result<Self> {
        wirt::WasmRuntime::new().map(Self)
    }

    /// Load a WASM component from a file.
    pub fn load_module(&self, id: String, path: &Path) -> Result<LoadedPlugin> {
        self.0.load_component(id, path).map(LoadedPlugin::from)
    }

    /// Load a WASM component from bytes.
    pub fn load_module_from_bytes(&self, id: String, bytes: &[u8]) -> Result<LoadedPlugin> {
        self.0
            .load_component_from_bytes(id, bytes)
            .map(LoadedPlugin::from)
    }
}

/// A loaded WASM plugin ready for execution.
pub struct LoadedPlugin {
    pub id: String,
    inner: wirt::LoadedComponent,
}

impl From<wirt::LoadedComponent> for LoadedPlugin {
    fn from(inner: wirt::LoadedComponent) -> Self {
        let id = inner.id().to_string();
        Self { id, inner }
    }
}

impl LoadedPlugin {
    /// Instantiate a plugin with its host-function state.
    ///
    /// The active-tab bridge is installed before Wirt instantiates the
    /// component so host imports can observe it from the first guest call.
    pub fn instantiate(
        &self,
        capabilities: Vec<PluginCapability>,
        requests_per_minute: u32,
        settings: HashMap<String, String>,
        active_tab_bridge: Option<Arc<dyn crate::ActiveTabBridge>>,
    ) -> Result<PluginInstance> {
        let host = HostFunctions::new(
            self.id.clone(),
            capabilities.into_iter().collect(),
            requests_per_minute,
            settings,
        )?;
        self.instantiate_with_host_functions(host, active_tab_bridge)
    }

    pub(crate) fn instantiate_with_plugin_log_dir(
        &self,
        capabilities: Vec<PluginCapability>,
        requests_per_minute: u32,
        settings: HashMap<String, String>,
        active_tab_bridge: Option<Arc<dyn crate::ActiveTabBridge>>,
        plugin_log_dir: &Path,
    ) -> Result<PluginInstance> {
        let host = HostFunctions::new_with_plugin_log_dir(
            self.id.clone(),
            capabilities.into_iter().collect(),
            requests_per_minute,
            settings,
            plugin_log_dir,
        )?;
        self.instantiate_with_host_functions(host, active_tab_bridge)
    }

    pub(crate) fn instantiate_for_metadata_validation(&self) -> Result<PluginInstance> {
        let host = HostFunctions::new_for_metadata_validation(self.id.clone())?;
        self.instantiate_with_host_functions(host, None)
    }

    fn instantiate_with_host_functions(
        &self,
        mut host: HostFunctions,
        active_tab_bridge: Option<Arc<dyn crate::ActiveTabBridge>>,
    ) -> Result<PluginInstance> {
        host.active_tab = active_tab_bridge;
        self.inner
            .instantiate(host)
            .map(|inner| PluginInstance { inner })
    }
}

/// An instantiated plugin that can receive events.
pub struct PluginInstance {
    inner: wirt::PluginInstance<HostFunctions>,
}

impl PluginInstance {
    /// Return the host-generated reason this instance was made terminal.
    pub fn unavailable_reason(&self) -> Option<&str> {
        self.inner.unavailable_reason()
    }

    /// Initialize the plugin.
    pub(crate) fn init(&mut self) -> Result<()> {
        self.inner.init()
    }

    /// Get plugin metadata, cached by the Wirt instance.
    pub(crate) fn get_metadata(&mut self) -> Result<PluginMetadata> {
        self.inner.get_metadata()
    }

    /// Get product-neutral rule definitions for the executor boundary.
    pub(crate) fn get_default_rule_definitions(
        &mut self,
    ) -> Result<Vec<wirt::PluginRuleDefinition>> {
        self.inner.get_default_rules()
    }

    /// Get UI layout for a specific extension point.
    pub(crate) fn get_ui_layout(
        &mut self,
        extension_point: PluginExtensionPoint,
    ) -> Result<crate::types::PluginLayout> {
        self.inner.get_ui_layout(&extension_point)
    }

    /// Send a UI event to the plugin and get actions back.
    pub(crate) fn send_ui_event(
        &mut self,
        element_id: &str,
        value: Option<String>,
    ) -> Result<Vec<crate::types::PluginAction>> {
        self.inner.send_ui_event(element_id, value)
    }

    /// Clean up the plugin.
    pub(crate) fn cleanup(&mut self) -> Result<()> {
        self.inner.cleanup()
    }

    /// Set the content cache for host functions.
    pub fn set_content_cache(&mut self, cache: Option<Arc<arclain_data::ContentCache>>) {
        let host = self.inner.host_state_mut();
        match cache {
            Some(cache) => host.set_content_cache(cache),
            None => host.content_cache = None,
        }
    }

    /// Set the resource manager for host functions.
    pub fn set_resource_manager(&mut self, manager: Option<Arc<arclain_data::ResourceManager>>) {
        let host = self.inner.host_state_mut();
        match manager {
            Some(manager) => host.set_resource_manager(manager),
            None => host.resource_manager = None,
        }
    }

    /// Install the bridge to the host's per-tab signal tree.
    pub fn set_active_tab_bridge(&mut self, bridge: Arc<dyn crate::ActiveTabBridge>) {
        self.inner.host_state_mut().set_active_tab_bridge(bridge);
    }

    /// Install (or clear) the per-event context for this instance.
    pub fn set_event_context(&mut self, ctx: Option<crate::host_functions::EventContext>) {
        self.inner.host_state_mut().set_event_context(ctx);
    }

    #[cfg(test)]
    pub(crate) fn has_event_context_for_test(&self) -> bool {
        self.inner.host_state().event_context.is_some()
    }

    /// Set the async HTTP client for host functions.
    pub fn set_async_http_client(&mut self, client: Option<Arc<arclain_network::AsyncHttpClient>>) {
        let host = self.inner.host_state_mut();
        match client {
            Some(client) => host.set_async_http_client(client),
            None => host.async_http_client = None,
        }
    }

    /// Set the library service for host functions.
    #[cfg(feature = "gameta")]
    pub fn set_library_service(&mut self, lib_svc: Option<Arc<arclain_core::LibraryService>>) {
        let host = self.inner.host_state_mut();
        match lib_svc {
            Some(lib_svc) => host.set_library_service(lib_svc),
            None => host.library_service = None,
        }
    }

    /// Set the gameta server client for host functions.
    pub fn set_gameta_client(
        &mut self,
        client: Option<Arc<arclain_network::features::gameta_client::GametaClient>>,
    ) {
        let host = self.inner.host_state_mut();
        match client {
            Some(client) => host.set_gameta_client(client),
            None => host.gameta_client = None,
        }
    }

    /// Get gameta client reference (if configured).
    pub fn get_gameta_client(
        &self,
    ) -> Option<Arc<arclain_network::features::gameta_client::GametaClient>> {
        self.inner.host_state().gameta_client.clone()
    }

    pub fn try_acquire_network_host_service(
        &self,
        service_scope: &str,
    ) -> std::result::Result<(), String> {
        let host = self.inner.host_state();
        let client = host
            .async_http_client
            .as_ref()
            .ok_or_else(|| "plugin network policy client unavailable".to_string())?;
        client
            .try_acquire_plugin_host_service(host.plugin_id.as_str(), service_scope)
            .map_err(|error| error.to_string())
    }

    pub fn data_materialization_limit(&self) -> usize {
        self.inner.host_state().data_service.materialization_limit()
    }

    /// Check the immutable manifest capabilities attached to this instance.
    pub fn has_capabilities(&self, required: &[PluginCapability]) -> bool {
        self.inner.host_state().has_capabilities(required)
    }

    /// Get a handle to the active-tab bridge if one has been installed.
    pub fn get_active_tab_bridge(&self) -> Option<Arc<dyn crate::ActiveTabBridge>> {
        self.inner.host_state().active_tab.clone()
    }

    /// Get network logs from the plugin.
    pub fn get_network_log(&self) -> Vec<(std::time::SystemTime, String)> {
        self.inner.host_state().network_log.lock().clone()
    }

    /// Get current settings from the plugin.
    pub fn get_settings(&self) -> Option<HashMap<String, String>> {
        Some(self.inner.host_state().settings.lock().clone())
    }

    /// Cheap clone of the plugin's `settings_dirty` flag.
    pub fn settings_dirty_handle(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.inner.host_state().settings_dirty.clone()
    }

    /// Get top-level tabs registered by this plugin.
    pub(crate) fn get_top_tabs(&mut self) -> Result<Vec<crate::types::TopTabConfig>> {
        self.inner.get_top_tabs()
    }
}

#[cfg(test)]
mod adapter_shape {
    use super::{HostFunctions, LoadedPlugin, PluginInstance, WasmRuntime};

    fn assert_runtime_is_exact_wirt_wrapper(adapter: WasmRuntime) {
        let WasmRuntime(inner) = adapter;
        let _: wirt::WasmRuntime = inner;
    }

    fn assert_loaded_plugin_is_exact_wirt_wrapper(adapter: LoadedPlugin) {
        let LoadedPlugin { id, inner } = adapter;
        let _: String = id;
        let _: wirt::LoadedComponent = inner;
    }

    fn assert_plugin_instance_is_exact_wirt_wrapper(adapter: PluginInstance) {
        let PluginInstance { inner } = adapter;
        let _: wirt::PluginInstance<HostFunctions> = inner;
    }

    const _: fn(WasmRuntime) = assert_runtime_is_exact_wirt_wrapper;
    const _: fn(LoadedPlugin) = assert_loaded_plugin_is_exact_wirt_wrapper;
    const _: fn(PluginInstance) = assert_plugin_instance_is_exact_wirt_wrapper;
}
