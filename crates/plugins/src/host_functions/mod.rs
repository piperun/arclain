//! Host functions that plugins can call
//!
//! This module implements the host-side functions that are exposed to WASM plugins
//! via the WASI Component Model.

mod archive;

mod logging;
mod metadata;
mod plugin_logger;
mod settings;
mod temp_storage;

#[cfg(test)]
mod tests;

pub use plugin_logger::PluginLogger;

pub(crate) fn bounded_plugin_settings(
    settings: HashMap<String, String>,
) -> HashMap<String, String> {
    settings::bounded_initial_settings(settings)
}

use crate::active_tab::ActiveTabBridge;
use crate::arclain::plugin::host::{Host, LogLevel};
use crate::types::{PluginCapability, PluginId, Result as PluginResult};
use arclain_data::DataService;
use arclain_data::ResourceManager;

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use wasmtime::component::ResourceTable;
use wasmtime::ResourceLimiter;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use temp_storage::PluginTempStorage;

const METADATA_VALIDATION_DENIED: &str = "metadata-validation-denied";
const EXTERNAL_LAUNCH_DENIED: &str = "external launch disabled: host UI authorization required";
const DATA_REQUEST_CAPABILITY_DENIED: &str = "host-data-request-capability-denied";

fn is_raw_metadata_cache_key(key: &str) -> bool {
    let Some((namespace, remainder)) = key.split_once(':') else {
        return false;
    };
    let Some((kind, payload)) = remainder.split_once(':') else {
        return false;
    };
    !namespace.is_empty()
        && !payload.is_empty()
        && ["json", "html", "metadata"]
            .iter()
            .any(|candidate| kind.eq_ignore_ascii_case(candidate))
}

fn sandboxed_wasi_ctx() -> WasiCtx {
    // No inherited stdio, argv, environment, or filesystem preopens. Guest
    // diagnostics must cross the bounded host logging API instead.
    WasiCtxBuilder::new().build()
}

pub(crate) const MAX_LINEAR_MEMORY_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const MAX_TABLE_ELEMENTS: usize = 100_000;
// `ResourceLimiter::instances` counts the component's adapter-internal core
// instances, not user-visible plugin instances. Each Store still owns exactly
// one `PluginWorld`; 32 accommodates the 20 core instances used by current
// components while bounding malformed components.
pub(crate) const MAX_CORE_INSTANCES: usize = 32;
pub(crate) const MAX_TABLES: usize = 8;
pub(crate) const MAX_MEMORIES: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StoreQuotaKind {
    Memory,
    Table,
}

#[derive(Debug)]
pub(crate) struct StoreQuotaExceeded {
    pub(crate) kind: StoreQuotaKind,
}

impl std::fmt::Display for StoreQuotaExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("plugin store resource quota exceeded")
    }
}

impl std::error::Error for StoreQuotaExceeded {}

#[derive(Debug, Default)]
pub(crate) struct PluginStoreLimiter;

impl ResourceLimiter for PluginStoreLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > MAX_LINEAR_MEMORY_BYTES {
            return Err(wasmtime::Error::new(StoreQuotaExceeded {
                kind: StoreQuotaKind::Memory,
            }));
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > MAX_TABLE_ELEMENTS {
            return Err(wasmtime::Error::new(StoreQuotaExceeded {
                kind: StoreQuotaKind::Table,
            }));
        }
        Ok(true)
    }

    fn instances(&self) -> usize {
        MAX_CORE_INSTANCES
    }

    fn tables(&self) -> usize {
        MAX_TABLES
    }

    fn memories(&self) -> usize {
        MAX_MEMORIES
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostMode {
    Normal,
    MetadataValidation,
}

/// Per-event context the dispatch worker installs while a plugin's
/// event handler runs. Carries snapshots of the *originating tab's*
/// state (archive path, password, entries list, metadata signal),
/// so host-function calls inside the handler resolve to the tab the
/// event was fired for — never to whatever tab is currently active
/// when the worker processes the event.
#[derive(Clone)]
pub struct EventContext {
    pub archive_path: String,
    pub password: Option<String>,
    pub entries: Arc<Vec<arclain_core::ArchiveEntry>>,
    pub metadata_signal: arclain_signals::Signal<Option<serde_json::Value>>,
}

/// State for host functions
pub struct HostFunctions {
    mode: HostMode,
    pub plugin_id: PluginId,
    pub async_http_client: Option<Arc<arclain_network::AsyncHttpClient>>,
    pub capabilities: std::collections::HashSet<PluginCapability>,
    requests_per_minute: u32,
    pub settings: Arc<Mutex<HashMap<String, String>>>,
    /// Flips to `true` whenever the plugin (or host) writes a setting.
    /// `PluginManager::get_all_settings` swaps this back to `false` after
    /// snapshotting, so subsequent calls can skip the lock + clone for
    /// untouched plugins (audit P14). Initialized to `true` so the very
    /// first snapshot populates the manager-side cache.
    pub settings_dirty: Arc<AtomicBool>,
    pub network_log: Arc<Mutex<Vec<(std::time::SystemTime, String)>>>,
    metadata_write_budget: metadata::MetadataWriteBudget,
    pub library_service: Option<Arc<arclain_core::LibraryService>>,
    pub content_cache: Option<Arc<arclain_data::ContentCache>>,
    pub gameta_client: Option<Arc<arclain_network::features::gameta_client::GametaClient>>,

    pub resource_manager: Option<Arc<ResourceManager>>,

    // Data API state
    pub data_service: DataService,
    pub table: ResourceTable,
    pub ctx: WasiCtx,
    pub(crate) store_limiter: PluginStoreLimiter,
    temp_storage: Option<PluginTempStorage>,

    /// Bridge to the host's per-tab signal tree — replaces the
    /// previous held `current_archive` / `current_password` /
    /// `metadata_signal` fields. See `crate::active_tab` for why
    /// this is a bridge instead of stored state.
    pub active_tab: Option<Arc<dyn ActiveTabBridge>>,

    /// Per-event override of the bridge. Set by the dispatch worker
    /// before calling a plugin's event handler, cleared after.
    /// While set, `current_archive_info` / `list_archive_files` /
    /// `emit_metadata` resolve through this context instead of the
    /// bridge, so the handler sees the tab the event was *fired
    /// for* even if events queued up and the user has switched tabs
    /// before the worker got around to processing. Non-event-time
    /// host calls (e.g. panel-render emits) still go through the
    /// bridge, which gives them the currently active tab — the
    /// correct semantic for plugin-UI-driven reads.
    pub event_context: Option<EventContext>,

    /// Per-plugin log file with rate limit + size cap. ERROR/WARN
    /// lines also escalate to arclain.log; INFO/DEBUG/TRACE go here
    /// only.
    pub plugin_logger: Arc<PluginLogger>,
}

/// Default location for per-plugin log files. Mirrors
/// `init_logging`'s arclain.log directory: `{data_dir}/arclain/logs/plugins`.
fn default_plugin_log_dir() -> std::path::PathBuf {
    arclain_core::utilities::plugin_log_dir()
}

impl HostFunctions {
    pub fn new(
        plugin_id: String,
        capabilities: std::collections::HashSet<PluginCapability>,
        requests_per_minute: u32,
        initial_settings: HashMap<String, String>,
    ) -> PluginResult<Self> {
        Self::new_with_plugin_log_dir(
            plugin_id,
            capabilities,
            requests_per_minute,
            initial_settings,
            &default_plugin_log_dir(),
        )
    }

    pub(crate) fn new_with_plugin_log_dir(
        plugin_id: String,
        capabilities: std::collections::HashSet<PluginCapability>,
        requests_per_minute: u32,
        initial_settings: HashMap<String, String>,
        plugin_log_dir: &Path,
    ) -> PluginResult<Self> {
        Self::build(
            plugin_id,
            capabilities,
            requests_per_minute,
            initial_settings,
            Some(plugin_log_dir),
            HostMode::Normal,
        )
    }

    pub(crate) fn new_for_metadata_validation(plugin_id: String) -> PluginResult<Self> {
        Self::build(
            plugin_id,
            Default::default(),
            0,
            HashMap::new(),
            None,
            HostMode::MetadataValidation,
        )
    }

    fn build(
        plugin_id: String,
        capabilities: std::collections::HashSet<PluginCapability>,
        requests_per_minute: u32,
        initial_settings: HashMap<String, String>,
        plugin_log_dir: Option<&Path>,
        mode: HostMode,
    ) -> PluginResult<Self> {
        let plugin_id = PluginId::parse(plugin_id)?;
        let initial_settings = bounded_plugin_settings(initial_settings);
        let ctx = sandboxed_wasi_ctx();

        let plugin_logger = Arc::new(match plugin_log_dir {
            Some(log_dir) => PluginLogger::new(&plugin_id, log_dir),
            None => PluginLogger::deferred(&plugin_id),
        });
        let data_service = DataService::new().with_id(plugin_id.as_str());
        data_service.set_materialization_limit(crate::types::MAX_PLUGIN_GUEST_DATA_BYTES);
        Ok(Self {
            mode,
            plugin_id,
            async_http_client: None,
            capabilities,
            requests_per_minute,
            settings: Arc::new(Mutex::new(initial_settings)),
            settings_dirty: Arc::new(AtomicBool::new(true)),
            network_log: Arc::new(Mutex::new(Vec::new())),
            metadata_write_budget: metadata::MetadataWriteBudget::default(),
            library_service: None,
            content_cache: None,
            gameta_client: None,

            resource_manager: None,

            data_service,
            table: ResourceTable::new(),
            ctx,
            store_limiter: PluginStoreLimiter,
            // Created only after an authorized `create_file` call. Loading a
            // plugin, including one with FileWrite, performs no temp I/O.
            temp_storage: None,
            active_tab: None,
            event_context: None,
            plugin_logger,
        })
    }

    fn is_metadata_validation(&self) -> bool {
        self.mode == HostMode::MetadataValidation
    }

    pub fn set_library_service(&mut self, lib_svc: Arc<arclain_core::LibraryService>) {
        // Register MetadataStore resolver with DataService
        let resolver = Arc::new(arclain_data::MetadataStoreResolver::new(
            lib_svc.clone() as Arc<dyn arclain_data::MetadataReader>
        ));

        self.data_service
            .register_resolver(arclain_data::DataSource::MetadataStore, resolver);
        self.library_service = Some(lib_svc);
    }

    pub fn set_content_cache(&mut self, cache: Arc<arclain_data::ContentCache>) {
        self.content_cache = Some(cache);
    }

    pub fn set_gameta_client(
        &mut self,
        client: Arc<arclain_network::features::gameta_client::GametaClient>,
    ) {
        self.gameta_client = Some(client);
    }

    /// Install the bridge to the host's per-tab signal tree. Replaces
    /// the pre-bridge `set_metadata_signal` + `set_archive_context`
    /// pair — see `crate::active_tab` for the rationale.
    pub fn set_active_tab_bridge(&mut self, bridge: Arc<dyn ActiveTabBridge>) {
        self.active_tab = Some(bridge);
    }

    /// Install (or clear) the per-event context. Called by the
    /// dispatch worker around a plugin event handler so all
    /// host-function reads inside the handler resolve to the
    /// originating tab's snapshot, not to the bridge.
    pub fn set_event_context(&mut self, ctx: Option<EventContext>) {
        self.event_context = ctx;
    }

    pub fn check_capability(&self, cap: PluginCapability) -> bool {
        self.capabilities.contains(&cap)
    }

    pub fn has_capabilities(&self, required: &[PluginCapability]) -> bool {
        required
            .iter()
            .all(|capability| self.check_capability(*capability))
    }

    fn require_capability(
        &self,
        capability: PluginCapability,
        operation: &str,
    ) -> std::result::Result<(), String> {
        if self.check_capability(capability) {
            return Ok(());
        }

        Err(format!(
            "{capability:?} capability not granted for {operation}"
        ))
    }

    pub fn set_async_http_client(&mut self, client: Arc<arclain_network::AsyncHttpClient>) {
        client.configure_plugin(
            self.plugin_id.as_str(),
            arclain_network::PluginNetworkPolicy {
                network_enabled: self.capabilities.contains(&PluginCapability::Network),
                requests_per_minute: self.requests_per_minute,
            },
        );
        // Register Network resolver with DataService
        let resolver = Arc::new(arclain_data::NetworkResolver::for_plugin(
            client.clone(),
            self.plugin_id.as_str(),
        ));
        self.data_service
            .register_resolver(arclain_data::DataSource::Network, resolver);
        self.async_http_client = Some(client);
    }

    pub fn set_resource_manager(&mut self, manager: Arc<ResourceManager>) {
        self.data_service.set_materialization_limit(
            manager
                .materialization_limit()
                .min(crate::types::MAX_PLUGIN_GUEST_DATA_BYTES),
        );
        // Register ContentCache resolver with DataService
        let resolver = Arc::new(arclain_data::ContentCacheResolver::new(manager.clone()));
        self.data_service
            .register_resolver(arclain_data::DataSource::ContentCache, resolver);
        self.resource_manager = Some(manager);
    }

    /// Translate a WIT-side `DataRequest` into the internal
    /// `arclain_data::DataRequest`, applying the same source-chain
    /// defaults that the Data API has always used. Shared by both
    /// `request_data` (returns bytes) and `fetch_to_cache` (drops
    /// bytes); the only difference between them is whether the
    /// resolver's result body crosses back over the WASM boundary.
    fn build_data_request(
        &self,
        request: crate::arclain::plugin::host::DataRequest,
    ) -> std::result::Result<arclain_data::DataRequest, String> {
        use arclain_data::{DataSource, ResourceType};

        let resource_type = match request.resource_type {
            crate::arclain::plugin::host::ResourceType::Binary => ResourceType::Binary,
            crate::arclain::plugin::host::ResourceType::Image => ResourceType::Image,
            crate::arclain::plugin::host::ResourceType::Json => ResourceType::Metadata,
        };

        let mut req = self
            .plugin_scoped_data_request(&request.key)
            .with_type(resource_type)
            .with_store_sources([]);

        tracing::debug!(
            plugin_id = %self.plugin_id.as_str(),
            has_url = request.url.is_some(),
            requested_sources = request.sources.len(),
            "Building plugin data request",
        );

        let explicit_sources = !request.sources.is_empty();
        let wants_metadata_store = if explicit_sources {
            request.sources.iter().any(|source| {
                matches!(
                    source,
                    crate::arclain::plugin::host::DataSource::MetadataCache
                )
            })
        } else {
            resource_type == ResourceType::Metadata
        };
        let wants_content_cache = !explicit_sources
            || request.sources.iter().any(|source| {
                matches!(
                    source,
                    crate::arclain::plugin::host::DataSource::ContentCache
                )
            });
        let wants_memory_store = request
            .sources
            .iter()
            .any(|source| matches!(source, crate::arclain::plugin::host::DataSource::Memory));

        if let Some(url) = request.url {
            req = req.with_url(url);
        }
        if let Some(pid) = request.product_id {
            req = req.with_product(pid);
        }

        let mut sources = arclain_data::IndexSet::new();
        if !request.sources.is_empty() {
            for src in request.sources {
                let (ds, allowed) = match src {
                    crate::arclain::plugin::host::DataSource::MetadataCache => (
                        DataSource::MetadataStore,
                        self.check_capability(PluginCapability::ArchiveMetadataRead),
                    ),
                    crate::arclain::plugin::host::DataSource::ContentCache => (
                        DataSource::ContentCache,
                        self.check_capability(PluginCapability::FileRead)
                            && ((resource_type != ResourceType::Metadata
                                && !is_raw_metadata_cache_key(&request.key))
                                || self.check_capability(PluginCapability::ArchiveMetadataRead)),
                    ),
                    crate::arclain::plugin::host::DataSource::LocalFile => (
                        DataSource::LocalFile,
                        self.check_capability(PluginCapability::FileRead),
                    ),
                    crate::arclain::plugin::host::DataSource::Memory => (DataSource::Memory, true),
                    crate::arclain::plugin::host::DataSource::Network => (
                        DataSource::Network,
                        self.check_capability(PluginCapability::Network),
                    ),
                };
                if allowed {
                    sources.insert(ds);
                }
            }
        } else {
            if resource_type == ResourceType::Metadata
                && self.check_capability(PluginCapability::ArchiveMetadataRead)
            {
                sources.insert(DataSource::MetadataStore);
            }
            if self.check_capability(PluginCapability::FileRead)
                && ((resource_type != ResourceType::Metadata
                    && !is_raw_metadata_cache_key(&request.key))
                    || self.check_capability(PluginCapability::ArchiveMetadataRead))
            {
                sources.insert(DataSource::ContentCache);
            }
            if self.check_capability(PluginCapability::Network) {
                sources.insert(DataSource::Network);
            }
        }

        if sources.is_empty() {
            return Err(
                "no requested data source is authorized by the plugin manifest".to_string(),
            );
        }

        let mut store_sources = Vec::with_capacity(3);
        if wants_metadata_store && self.check_capability(PluginCapability::ArchiveMetadataWrite) {
            store_sources.push(DataSource::MetadataStore);
        }
        if wants_content_cache
            && self.check_capability(PluginCapability::FileWrite)
            && (resource_type != ResourceType::Metadata && !is_raw_metadata_cache_key(&request.key)
                || self.check_capability(PluginCapability::ArchiveMetadataWrite))
        {
            store_sources.push(DataSource::ContentCache);
        }
        if wants_memory_store {
            store_sources.push(DataSource::Memory);
        }
        req = req.with_sources(sources).with_store_sources(store_sources);
        Ok(req)
    }

    fn plugin_scoped_data_request(&self, key: &str) -> arclain_data::DataRequest {
        arclain_data::DataRequest::new(key).with_plugin_id(self.plugin_id.as_str())
    }

    fn readable_cache_request(&self, key: &str) -> arclain_data::DataRequest {
        let mut sources = Vec::with_capacity(3);
        if self.check_capability(PluginCapability::ArchiveMetadataRead) {
            sources.push(arclain_data::DataSource::MetadataStore);
        }
        if self.check_capability(PluginCapability::FileRead)
            && (!is_raw_metadata_cache_key(key)
                || self.check_capability(PluginCapability::ArchiveMetadataRead))
        {
            sources.push(arclain_data::DataSource::ContentCache);
        }
        sources.push(arclain_data::DataSource::Memory);
        self.plugin_scoped_data_request(key)
            .with_sources(sources)
            .with_store_sources([])
    }

    pub(super) fn with_authorized_gameta_request<T>(
        &self,
        request: impl FnOnce(usize) -> T,
    ) -> Option<T> {
        if !self.check_capability(PluginCapability::Network) {
            return None;
        }
        let client = self.async_http_client.as_ref()?;
        client
            .try_acquire_plugin_host_service(self.plugin_id.as_str(), "gameta")
            .ok()?;
        Some(request(
            self.data_service
                .materialization_limit()
                .min(crate::types::MAX_PLUGIN_METADATA_BYTES),
        ))
    }

    /// Create a collision-safe file in this plugin instance's private temp directory.
    pub(super) fn impl_create_file(
        &mut self,
        filename: String,
        content: Vec<u8>,
    ) -> Result<String, String> {
        if self.temp_storage.is_none() {
            self.temp_storage =
                Some(PluginTempStorage::new().map_err(|error| {
                    format!("failed to create plugin temporary storage: {error}")
                })?);
        }
        let storage = self
            .temp_storage
            .as_mut()
            .expect("storage initialized above");
        let path = storage.create_file(&filename, &content)?;
        tracing::debug!(
            plugin_id = %self.plugin_id.as_str(),
            bytes = content.len(),
            "Created plugin-owned temporary file"
        );
        Ok(path.to_string_lossy().into_owned())
    }
}

// Implement WasiView for HostFunctions
impl WasiView for HostFunctions {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

impl Host for HostFunctions {
    fn log(&mut self, level: LogLevel, message: String) {
        if self.is_metadata_validation() {
            return;
        }
        self.impl_log(level, message)
    }

    fn log_network_activity(&mut self, message: String) {
        if self.is_metadata_validation() {
            return;
        }
        self.impl_log_network_activity(message)
    }

    fn get_setting(&mut self, key: String) -> Option<String> {
        if self.is_metadata_validation() {
            return None;
        }
        self.impl_get_setting(key)
    }

    fn set_setting(&mut self, key: String, value: String) {
        if self.is_metadata_validation() {
            return;
        }
        self.impl_set_setting(key, value)
    }

    fn current_archive_info(&mut self) -> Option<crate::arclain::plugin::host::ArchiveInfo> {
        if self.is_metadata_validation() {
            return None;
        }
        self.require_capability(
            PluginCapability::ArchiveMetadataRead,
            "current_archive_info",
        )
        .ok()?;
        self.impl_current_archive_info()
    }

    fn list_archive_files(&mut self) -> std::result::Result<Vec<String>, String> {
        if self.is_metadata_validation() {
            return Err(METADATA_VALIDATION_DENIED.to_string());
        }
        self.require_capability(PluginCapability::ArchiveMetadataRead, "list_archive_files")?;
        self.impl_list_archive_files()
    }

    fn archive_file_count(&mut self) -> std::result::Result<u64, String> {
        if self.is_metadata_validation() {
            return Err(METADATA_VALIDATION_DENIED.to_string());
        }
        self.require_capability(PluginCapability::ArchiveMetadataRead, "archive_file_count")?;
        self.impl_archive_file_count()
    }

    fn list_archive_files_page(
        &mut self,
        offset: u32,
        limit: u32,
    ) -> std::result::Result<Vec<String>, String> {
        if self.is_metadata_validation() {
            return Err(METADATA_VALIDATION_DENIED.to_string());
        }
        self.require_capability(
            PluginCapability::ArchiveMetadataRead,
            "list_archive_files_page",
        )?;
        self.impl_list_archive_files_page(offset, limit)
    }

    fn rename_archive(&mut self, new_name: String) -> std::result::Result<String, String> {
        if self.is_metadata_validation() {
            return Err(METADATA_VALIDATION_DENIED.to_string());
        }
        self.require_capability(PluginCapability::ArchiveModify, "rename_archive")?;
        self.impl_rename_archive(new_name)
    }

    fn emit_metadata(&mut self, metadata_json: String) {
        if self.is_metadata_validation() {
            return;
        }
        if self
            .require_capability(PluginCapability::ArchiveMetadataWrite, "emit_metadata")
            .is_err()
        {
            return;
        }
        let _ = self.impl_emit_metadata(metadata_json);
    }

    fn emit_metadata_for_source(&mut self, source: String, metadata_json: String) -> bool {
        if self.is_metadata_validation()
            || self
                .require_capability(
                    PluginCapability::ArchiveMetadataWrite,
                    "emit_metadata_for_source",
                )
                .is_err()
        {
            return false;
        }
        self.impl_emit_metadata_for_source(source, metadata_json)
    }

    fn show_message(&mut self, title: String, message: String) {
        if self.is_metadata_validation() {
            return;
        }
        self.impl_show_message(title, message)
    }

    fn set_status_message(&mut self, message: String) {
        if self.is_metadata_validation() {
            return;
        }
        // Store the status bar message for the UI to pick up
        // Retained status messages never had a production reader. Keep the
        // WIT import for ABI compatibility without retaining guest memory.
        let _ = message;
    }

    fn list_cached_entries(&mut self) -> Vec<String> {
        if self.is_metadata_validation() {
            return Vec::new();
        }
        if self
            .require_capability(PluginCapability::ArchiveMetadataRead, "list_cached_entries")
            .is_err()
        {
            return Vec::new();
        }
        self.impl_list_cached_entries()
    }

    fn cached_metadata_count(&mut self, source: String) -> Result<u64, String> {
        if self.is_metadata_validation() {
            return Err(METADATA_VALIDATION_DENIED.to_string());
        }
        self.require_capability(
            PluginCapability::ArchiveMetadataRead,
            "cached_metadata_count",
        )?;
        self.impl_cached_metadata_count(source)
    }

    fn list_cached_metadata(
        &mut self,
        source: String,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<String>, String> {
        if self.is_metadata_validation() {
            return Err(METADATA_VALIDATION_DENIED.to_string());
        }
        self.require_capability(
            PluginCapability::ArchiveMetadataRead,
            "list_cached_metadata",
        )?;
        self.impl_list_cached_metadata(source, offset, limit)
    }

    fn get_metadata_summaries(
        &mut self,
        ids: Vec<String>,
    ) -> Vec<crate::arclain::plugin::host::MetadataSummary> {
        if self.is_metadata_validation() {
            return Vec::new();
        }
        if self
            .require_capability(
                PluginCapability::ArchiveMetadataRead,
                "get_metadata_summaries",
            )
            .is_err()
        {
            return Vec::new();
        }
        self.impl_get_metadata_summaries(ids)
    }

    fn get_metadata_summaries_for_source(
        &mut self,
        source: String,
        ids: Vec<String>,
    ) -> Result<Vec<crate::arclain::plugin::host::MetadataSummary>, String> {
        if self.is_metadata_validation() {
            return Err(METADATA_VALIDATION_DENIED.to_string());
        }
        self.require_capability(
            PluginCapability::ArchiveMetadataRead,
            "get_metadata_summaries_for_source",
        )?;
        self.impl_get_metadata_summaries_for_source(source, ids)
    }

    fn get_product_metadata(&mut self, product_id: String, source: String) -> Option<String> {
        if self.is_metadata_validation() {
            return None;
        }
        self.require_capability(
            PluginCapability::ArchiveMetadataRead,
            "get_product_metadata",
        )
        .ok()?;
        self.impl_get_product_metadata(product_id, source)
    }

    // === Data API (unified) ===
    fn request_data(&mut self, request: crate::arclain::plugin::host::DataRequest) -> String {
        if self.is_metadata_validation() {
            return METADATA_VALIDATION_DENIED.to_string();
        }
        match self.build_data_request(request) {
            Ok(req) => self.data_service.request_data(req),
            Err(_) => DATA_REQUEST_CAPABILITY_DENIED.to_string(),
        }
    }

    fn fetch_to_cache(&mut self, request: crate::arclain::plugin::host::DataRequest) -> bool {
        if self.is_metadata_validation() {
            return false;
        }
        if !self.has_capabilities(&[PluginCapability::Network, PluginCapability::FileWrite]) {
            return false;
        }
        let metadata_write_required = matches!(
            request.resource_type,
            crate::arclain::plugin::host::ResourceType::Json
        ) || is_raw_metadata_cache_key(&request.key);
        if metadata_write_required && !self.check_capability(PluginCapability::ArchiveMetadataWrite)
        {
            return false;
        }
        let req = match self.build_data_request(request) {
            Ok(req) if req.sources.contains(&arclain_data::DataSource::Network) => req,
            Ok(_) | Err(_) => return false,
        };

        // Fast path: when we have a URL and both ContentCache and
        // AsyncHttpClient wired up, use the streaming download
        // pipeline. Bytes flow HTTP → .partial → cacache without ever
        // landing in a host-resident Vec<u8>, and a failed transfer
        // leaves the .partial file behind for the next call to resume
        // from. This is the path the chobit-video pipeline depends on
        // for 1 GB+ blobs — see the project_dlsite_video memory.
        //
        // The slow path (data_service.resolve) is kept for the case
        // where one of the dependencies is missing (early init, tests
        // without a network client, etc.) or the request has no URL
        // (cache-only check). It still buffers in memory, but those
        // paths only see small JSON / image bodies.
        if let (Some(url), Some(cache), Some(http_client)) = (
            req.url.as_deref(),
            self.content_cache.as_ref(),
            self.async_http_client.as_ref(),
        ) {
            let key = req.key.clone();
            let cache_type = match req.resource_type {
                arclain_data::ResourceType::Binary => arclain_db::CacheType::Other,
                arclain_data::ResourceType::Image => arclain_db::CacheType::Cover,
                arclain_data::ResourceType::Metadata => arclain_db::CacheType::Other,
                arclain_data::ResourceType::Text => arclain_db::CacheType::Other,
            };
            let product_id = req.product_id.as_deref();
            match arclain_data::features::streaming_download::fetch_url_to_cache_for_plugin(
                cache,
                http_client,
                &key,
                url,
                cache_type,
                product_id,
                self.plugin_id.as_str(),
            ) {
                Ok(bytes) => {
                    tracing::debug!(
                        streamed_bytes = bytes,
                        "Plugin fetch-to-cache streaming request completed"
                    );
                    return true;
                }
                Err(_) => {
                    tracing::warn!("Plugin fetch-to-cache streaming request failed");
                    return false;
                }
            }
        }

        // Fallback: buffered resolve through the DataService chain.
        let result = self.data_service.resolve(&req);
        let ok = matches!(
            result.status,
            arclain_data::DataStatus::Ready | arclain_data::DataStatus::Cached,
        );
        if ok {
            tracing::debug!(
                buffered_bytes = result.data.as_ref().map(|data| data.len()).unwrap_or(0),
                "Plugin fetch-to-cache buffered request completed",
            );
        } else {
            tracing::warn!("Plugin fetch-to-cache buffered request failed");
        }
        ok
    }

    fn play_cached_blob(
        &mut self,
        _key: String,
        _extension: String,
    ) -> std::result::Result<(), String> {
        if self.is_metadata_validation() {
            return Err(METADATA_VALIDATION_DENIED.to_string());
        }
        self.require_capability(PluginCapability::FileRead, "play_cached_blob")?;

        // A plugin callback is not proof of a current user gesture. Keep the
        // ABI for compatibility, but fail closed until the host UI can mint a
        // non-forgeable, single-use authorization for a validated media item.
        Err(EXTERNAL_LAUNCH_DENIED.to_string())
    }

    fn poll_data(&mut self, request_id: String) -> crate::arclain::plugin::host::DataResult {
        if self.is_metadata_validation() {
            return crate::arclain::plugin::host::DataResult {
                status: crate::arclain::plugin::host::DataStatus::Failed,
                data: None,
                error: Some(METADATA_VALIDATION_DENIED.to_string()),
            };
        }
        if request_id == DATA_REQUEST_CAPABILITY_DENIED {
            return crate::arclain::plugin::host::DataResult {
                status: crate::arclain::plugin::host::DataStatus::Failed,
                data: None,
                error: Some(
                    "no requested data source is authorized by the plugin manifest".to_string(),
                ),
            };
        }
        tracing::debug!("Plugin data request polling started");
        let result = self.data_service.poll_data(&request_id);
        if result
            .data
            .as_ref()
            .is_some_and(|data| data.len() > crate::types::MAX_PLUGIN_GUEST_DATA_BYTES)
        {
            return crate::arclain::plugin::host::DataResult {
                status: crate::arclain::plugin::host::DataStatus::Failed,
                data: None,
                error: Some("data response exceeds plugin guest-return limit".to_string()),
            };
        }
        tracing::debug!(
            status = ?result.status,
            has_data = result.data.is_some(),
            has_error = result.error.is_some(),
            "Plugin data request polled"
        );

        // Map arclain_data::DataResult to buf-generated DataResult
        use crate::arclain::plugin::host::{DataResult, DataStatus};
        DataResult {
            status: match result.status {
                arclain_data::DataStatus::Pending => DataStatus::Pending,
                arclain_data::DataStatus::Fetching => DataStatus::Fetching,
                arclain_data::DataStatus::Ready => DataStatus::Ready,
                arclain_data::DataStatus::Failed => DataStatus::Failed,
                arclain_data::DataStatus::Cached => DataStatus::Cached,
            },
            data: result.data,
            error: result.error,
        }
    }

    fn has_data(&mut self, key: String) -> bool {
        if self.is_metadata_validation() {
            return false;
        }
        let request = self.readable_cache_request(&key);
        self.data_service.has_data_for_request(&request)
    }

    fn get_data(&mut self, key: String) -> Option<Vec<u8>> {
        if self.is_metadata_validation() {
            return None;
        }
        let request = self.readable_cache_request(&key);
        self.data_service
            .get_data_for_request(&request)
            .filter(|data| data.len() <= crate::types::MAX_PLUGIN_GUEST_DATA_BYTES)
    }

    fn invalidate_cache(&mut self, key: String) -> bool {
        if self.is_metadata_validation() {
            return false;
        }
        if self
            .require_capability(PluginCapability::FileWrite, "invalidate_cache")
            .is_err()
        {
            return false;
        }
        let is_wildcard = key.ends_with('*');
        if (is_wildcard || is_raw_metadata_cache_key(&key))
            && self
                .require_capability(
                    PluginCapability::ArchiveMetadataWrite,
                    "invalidate_cache wildcard or raw metadata",
                )
                .is_err()
        {
            return false;
        }
        let owner = arclain_data::CacheOwner::plugin(self.plugin_id.as_str());
        tracing::debug!("Plugin cache invalidation requested");

        // Check for wildcard pattern
        if is_wildcard {
            tracing::debug!("Plugin cache wildcard invalidation requested");

            // Delete from content cache using pattern
            let mut count = 0;
            if let Some(cache) = &self.content_cache {
                if let Ok(c) = cache.remove_by_pattern_for_owner(&owner, &key) {
                    count = c;
                    tracing::debug!(
                        "[HostFunctions] Removed {} entries matching wildcard pattern",
                        count
                    );
                }
            }

            // LibraryService doesn't support wildcard deleting yet, but we only use this for content cache anyway
            return count > 0;
        }

        let mut invalidated = false;

        // Remove from content cache only (key format: dlsite:json:ID or dlsite:html:ID)
        // NOTE: This does NOT delete metadata entries - only cached content blobs.
        // Metadata entries should only be deleted via explicit delete actions.
        if let Some(cache) = &self.content_cache {
            if let Ok(true) = cache.remove_for_owner(&owner, &key) {
                tracing::debug!("Plugin content-cache entry invalidated");
                invalidated = true;
            }
        }

        invalidated
    }

    fn create_file(&mut self, filename: String, content: Vec<u8>) -> Result<String, String> {
        if self.is_metadata_validation() {
            return Err(METADATA_VALIDATION_DENIED.to_string());
        }
        self.require_capability(PluginCapability::FileWrite, "create_file")?;
        self.impl_create_file(filename, content)
    }
}

// Implement the ui::Host trait (empty - ui interface only defines types)
impl crate::arclain::plugin::ui::Host for HostFunctions {}

// Implement the rules::Host trait (empty - rules interface only defines types)
impl crate::arclain::plugin::rules::Host for HostFunctions {}

// Implement the meta::Host trait (empty - meta interface only defines types)
impl crate::arclain::plugin::meta::Host for HostFunctions {}

#[cfg(test)]
mod validation_mode_tests {
    use super::*;
    use crate::arclain::plugin::host::{DataRequest, DataSource, DataStatus, ResourceType};
    use tracing_test::traced_test;

    const FILE_SENTINEL: &str = "arclain-validation-host-boundary-sentinel.txt";

    fn data_request() -> DataRequest {
        DataRequest {
            key: "validation-key".to_string(),
            url: Some("https://example.invalid/validation".to_string()),
            resource_type: ResourceType::Binary,
            product_id: Some("validation-product".to_string()),
            sources: vec![DataSource::Network],
        }
    }

    #[traced_test]
    #[test]
    fn metadata_validation_denies_every_side_effecting_host_import() {
        let sentinel = std::env::temp_dir().join(FILE_SENTINEL);
        assert!(!sentinel.exists(), "test sentinel must start absent");

        let mut host =
            HostFunctions::new_for_metadata_validation("temp-validation".to_string()).unwrap();

        Host::log(
            &mut host,
            LogLevel::Warn,
            "validation-global-log-sentinel".to_string(),
        );
        Host::log_network_activity(&mut host, "validation-network-log-sentinel".to_string());
        Host::set_setting(&mut host, "persisted".to_string(), "mutation".to_string());
        Host::emit_metadata(
            &mut host,
            r#"{"product_id":"validation-metadata-sentinel"}"#.to_string(),
        );
        Host::show_message(
            &mut host,
            "validation-show-message-sentinel".to_string(),
            "must be suppressed".to_string(),
        );
        Host::set_status_message(&mut host, "validation-status-sentinel".to_string());

        assert_eq!(Host::get_setting(&mut host, "persisted".to_string()), None);
        assert!(Host::current_archive_info(&mut host).is_none());
        assert!(Host::list_archive_files(&mut host).is_err());
        assert!(Host::rename_archive(&mut host, "renamed.zip".to_string()).is_err());
        assert!(Host::list_cached_entries(&mut host).is_empty());
        assert!(Host::get_metadata_summaries(&mut host, vec!["id".to_string()]).is_empty());
        assert_eq!(
            Host::get_product_metadata(&mut host, "id".to_string(), "source".to_string()),
            None,
        );
        assert_eq!(
            Host::request_data(&mut host, data_request()),
            "metadata-validation-denied"
        );
        assert!(!Host::fetch_to_cache(&mut host, data_request()));
        assert!(Host::play_cached_blob(&mut host, "key".to_string(), "bin".to_string()).is_err());

        let polled = Host::poll_data(&mut host, "request".to_string());
        assert!(matches!(polled.status, DataStatus::Failed));
        assert!(polled.data.is_none());
        assert_eq!(polled.error.as_deref(), Some("metadata-validation-denied"));
        assert!(!Host::has_data(&mut host, "key".to_string()));
        assert_eq!(Host::get_data(&mut host, "key".to_string()), None);
        assert!(!Host::invalidate_cache(&mut host, "key".to_string()));
        assert!(
            Host::create_file(&mut host, FILE_SENTINEL.to_string(), b"owned".to_vec()).is_err()
        );

        assert!(
            !sentinel.exists(),
            "validation host must not create temp files"
        );
        assert!(host.settings.lock().is_empty());
        assert!(host.network_log.lock().is_empty());
        assert!(!logs_contain("validation-global-log-sentinel"));
        assert!(!logs_contain("validation-network-log-sentinel"));
        assert!(!logs_contain("validation-metadata-sentinel"));
        assert!(!logs_contain("validation-show-message-sentinel"));
        assert!(!logs_contain("validation-status-sentinel"));
    }
}
