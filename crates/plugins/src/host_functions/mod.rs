//! Host functions that plugins can call
//!
//! This module implements the host-side functions that are exposed to WASM plugins
//! via the WASI Component Model.

mod archive;

mod logging;
mod metadata;
mod settings;

use crate::arclain::plugin::host::{Host, LogLevel};
use crate::types::PluginCapability;
use arclain_core::ArchiveBackend;
use arclain_data::DataService;
use arclain_data::ResourceManager;

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};

/// State for host functions
pub struct HostFunctions {
    pub plugin_id: String,
    pub async_http_client: Option<Arc<arclain_http::AsyncHttpClient>>,
    pub capabilities: std::collections::HashSet<PluginCapability>,
    pub archive_backend: Option<Arc<dyn ArchiveBackend>>,
    pub current_archive: Arc<Mutex<Option<String>>>,
    pub current_password: Arc<Mutex<Option<String>>>,
    pub settings: Arc<Mutex<HashMap<String, String>>>,
    pub pending_messages: Arc<Mutex<Vec<(String, String)>>>,
    pub emitted_metadata: Arc<Mutex<Option<String>>>,
    pub network_log: Arc<Mutex<Vec<(std::time::SystemTime, String)>>>,
    pub metadata_cache: Option<Arc<arclain_db::MetadataCache>>,
    pub content_cache: Option<Arc<arclain_data::ContentCache>>,
    pub cache_db: Option<Arc<arclain_db::SqliteDb>>,

    pub resource_manager: Option<Arc<ResourceManager>>,

    // Data API state
    pub data_service: DataService,
    pub table: ResourceTable,
    pub ctx: WasiCtx,
}

impl HostFunctions {
    pub fn new(
        plugin_id: String,
        capabilities: std::collections::HashSet<PluginCapability>,
        _requests_per_minute: u32,
        initial_settings: HashMap<String, String>,
    ) -> Self {
        // Initialize WASI context
        let ctx = WasiCtxBuilder::new().inherit_stdio().inherit_args().build();

        Self {
            plugin_id,
            async_http_client: None,
            capabilities,
            archive_backend: None,
            current_archive: Arc::new(Mutex::new(None)),
            current_password: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(initial_settings)),
            pending_messages: Arc::new(Mutex::new(Vec::new())),
            emitted_metadata: Arc::new(Mutex::new(None)),
            network_log: Arc::new(Mutex::new(Vec::new())),
            metadata_cache: None,
            content_cache: None,
            cache_db: None,

            resource_manager: None,

            data_service: DataService::new(),
            table: ResourceTable::new(),
            ctx,
        }
    }

    pub fn with_backend(
        plugin_id: String,
        capabilities: std::collections::HashSet<PluginCapability>,
        requests_per_minute: u32,
        backend: Arc<dyn ArchiveBackend>,
        initial_settings: HashMap<String, String>,
    ) -> Self {
        let mut host_funcs = Self::new(
            plugin_id,
            capabilities,
            requests_per_minute,
            initial_settings,
        );
        host_funcs.archive_backend = Some(backend);
        host_funcs
    }

    pub fn set_metadata_cache(&mut self, cache: Arc<arclain_db::MetadataCache>) {
        // Register MetadataCache resolver with DataService
        let resolver = Arc::new(arclain_data::MetadataCacheResolver::new(cache.clone()));
        self.data_service
            .register_resolver(arclain_data::DataSource::MetadataCache, resolver);
        self.metadata_cache = Some(cache);
    }

    pub fn set_content_cache(&mut self, cache: Arc<arclain_data::ContentCache>) {
        self.content_cache = Some(cache);
    }

    pub fn set_cache_db(&mut self, db: Arc<arclain_db::SqliteDb>) {
        self.cache_db = Some(db);
    }

    pub fn set_archive_context(&self, archive_path: Option<String>, password: Option<String>) {
        *self.current_archive.lock() = archive_path;
        *self.current_password.lock() = password;
    }

    pub fn check_capability(&self, cap: PluginCapability) -> bool {
        self.capabilities.contains(&cap)
    }

    pub fn set_async_http_client(&mut self, client: Arc<arclain_http::AsyncHttpClient>) {
        // Register Network resolver with DataService
        let resolver = Arc::new(arclain_data::NetworkResolver::new(client.clone()));
        self.data_service
            .register_resolver(arclain_data::DataSource::Network, resolver);
        self.async_http_client = Some(client);
    }

    pub fn set_resource_manager(&mut self, manager: Arc<ResourceManager>) {
        // Register ContentCache resolver with DataService
        let resolver = Arc::new(arclain_data::ContentCacheResolver::new(manager.clone()));
        self.data_service
            .register_resolver(arclain_data::DataSource::ContentCache, resolver);
        self.resource_manager = Some(manager);
    }
}

// Implement WasiView for HostFunctions
impl WasiView for HostFunctions {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.ctx
    }
}

impl Host for HostFunctions {
    fn log(&mut self, level: LogLevel, message: String) {
        self.impl_log(level, message)
    }

    fn log_network_activity(&mut self, message: String) {
        self.impl_log_network_activity(message)
    }

    fn get_setting(&mut self, key: String) -> Option<String> {
        self.impl_get_setting(key)
    }

    fn set_setting(&mut self, key: String, value: String) {
        self.impl_set_setting(key, value)
    }

    fn current_archive_info(&mut self) -> Option<crate::arclain::plugin::host::ArchiveInfo> {
        self.impl_current_archive_info()
    }

    fn list_archive_files(&mut self) -> std::result::Result<Vec<String>, String> {
        self.impl_list_archive_files()
    }

    fn emit_metadata(&mut self, metadata_json: String) {
        self.impl_emit_metadata(metadata_json)
    }

    fn show_message(&mut self, title: String, message: String) {
        self.impl_show_message(title, message)
    }

    fn list_cached_entries(&mut self) -> Vec<String> {
        self.impl_list_cached_entries()
    }

    fn export_cache(&mut self) -> Result<String, String> {
        self.impl_export_cache()
    }

    fn import_cache(&mut self) -> Result<String, String> {
        self.impl_import_cache()
    }

    // === Data API (unified) ===
    // === Data API (unified) ===
    fn request_data(&mut self, request: crate::arclain::plugin::host::DataRequest) -> String {
        // Map buf-generated request to arclain_data::DataRequest
        use arclain_data::{DataRequest, DataSource, ResourceType};

        let resource_type = match request.resource_type {
            crate::arclain::plugin::host::ResourceType::Binary => ResourceType::Binary,
            crate::arclain::plugin::host::ResourceType::Image => ResourceType::Image,
            crate::arclain::plugin::host::ResourceType::Json => ResourceType::Metadata,
        };

        // Build request
        let mut req = DataRequest::new(&request.key).with_type(resource_type);

        // Set URL if provided
        if let Some(url) = request.url {
            req = req.with_url(url);
        }

        // Set product ID if provided
        if let Some(pid) = request.product_id {
            req = req.with_product(pid);
        }

        // Map WIT sources to internal DataSource
        if !request.sources.is_empty() {
            let mut sources = arclain_data::IndexSet::new();
            for src in request.sources {
                let ds = match src {
                    crate::arclain::plugin::host::DataSource::MetadataCache => {
                        DataSource::MetadataCache
                    }
                    crate::arclain::plugin::host::DataSource::ContentCache => {
                        DataSource::ContentCache
                    }
                    crate::arclain::plugin::host::DataSource::LocalFile => DataSource::LocalFile,
                    crate::arclain::plugin::host::DataSource::Memory => DataSource::Memory,
                    crate::arclain::plugin::host::DataSource::Network => DataSource::Network,
                };
                sources.insert(ds);
            }
            req = req.with_sources(sources);
        } else if resource_type == ResourceType::Metadata {
            // Default: for metadata type, check MetadataCache first
            let mut sources = arclain_data::IndexSet::new();
            sources.insert(DataSource::MetadataCache);
            sources.insert(DataSource::ContentCache);
            sources.insert(DataSource::Network);
            req = req.with_sources(sources);
        }

        self.data_service.request_data(req)
    }

    fn poll_data(&mut self, request_id: String) -> crate::arclain::plugin::host::DataResult {
        let result = self.data_service.poll_data(&request_id);

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
        self.data_service.has_data(&key)
    }

    fn get_data(&mut self, key: String) -> Option<Vec<u8>> {
        self.data_service.get_data(&key)
    }
}

// Implement the ui::Host trait (empty - ui interface only defines types)
impl crate::arclain::plugin::ui::Host for HostFunctions {}

// Implement the rules::Host trait (empty - rules interface only defines types)
impl crate::arclain::plugin::rules::Host for HostFunctions {}
