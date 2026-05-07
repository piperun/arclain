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
    pub async_http_client: Option<Arc<arclain_network::AsyncHttpClient>>,
    pub capabilities: std::collections::HashSet<PluginCapability>,
    pub archive_backend: Option<Arc<dyn ArchiveBackend>>,
    pub current_archive: Arc<Mutex<Option<String>>>,
    pub current_password: Arc<Mutex<Option<String>>>,
    pub settings: Arc<Mutex<HashMap<String, String>>>,
    pub pending_messages: Arc<Mutex<Vec<(String, String)>>>,
    pub emitted_metadata: Arc<Mutex<Option<String>>>,
    pub network_log: Arc<Mutex<Vec<(std::time::SystemTime, String)>>>,
    pub library_service: Option<Arc<arclain_core::LibraryService>>,
    pub content_cache: Option<Arc<arclain_data::ContentCache>>,
    pub gameta_client: Option<Arc<arclain_network::features::gameta_client::GametaClient>>,

    pub resource_manager: Option<Arc<ResourceManager>>,

    // Data API state
    pub data_service: DataService,
    pub table: ResourceTable,
    pub ctx: WasiCtx,

    // Reactive signal for UI updates
    pub metadata_signal: Option<arclain_signals::Signal<Option<serde_json::Value>>>,

    // Pending status message from plugin (to be displayed in status bar)
    pub pending_status_message: Arc<Mutex<Option<String>>>,

    // Pending clipboard text from plugin (to be copied by UI)
    pub pending_clipboard: Arc<Mutex<Option<String>>>,
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
            plugin_id: plugin_id.clone(),
            async_http_client: None,
            capabilities,
            archive_backend: None,
            current_archive: Arc::new(Mutex::new(None)),
            current_password: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(initial_settings)),
            pending_messages: Arc::new(Mutex::new(Vec::new())),
            emitted_metadata: Arc::new(Mutex::new(None)),
            network_log: Arc::new(Mutex::new(Vec::new())),
            library_service: None,
            content_cache: None,
            gameta_client: None,

            resource_manager: None,

            data_service: DataService::new().with_id(&plugin_id),
            table: ResourceTable::new(),
            ctx,
            metadata_signal: None,
            pending_status_message: Arc::new(Mutex::new(None)),
            pending_clipboard: Arc::new(Mutex::new(None)),
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

    pub fn set_library_service(&mut self, lib_svc: Arc<arclain_core::LibraryService>) {
        // Register MetadataStore resolver with DataService
        let resolver = Arc::new(arclain_data::MetadataStoreResolver::new(
            lib_svc.clone() as Arc<dyn arclain_data::MetadataReader>,
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

    pub fn set_metadata_signal(
        &mut self,
        signal: arclain_signals::Signal<Option<serde_json::Value>>,
    ) {
        self.metadata_signal = Some(signal);
    }

    pub fn set_archive_context(&self, archive_path: Option<String>, password: Option<String>) {
        *self.current_archive.lock() = archive_path;
        *self.current_password.lock() = password;
    }

    pub fn check_capability(&self, cap: PluginCapability) -> bool {
        self.capabilities.contains(&cap)
    }

    pub fn set_async_http_client(&mut self, client: Arc<arclain_network::AsyncHttpClient>) {
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

    /// Translate a WIT-side `DataRequest` into the internal
    /// `arclain_data::DataRequest`, applying the same source-chain
    /// defaults that the Data API has always used. Shared by both
    /// `request_data` (returns bytes) and `fetch_to_cache` (drops
    /// bytes); the only difference between them is whether the
    /// resolver's result body crosses back over the WASM boundary.
    fn build_data_request(
        &self,
        request: crate::arclain::plugin::host::DataRequest,
    ) -> arclain_data::DataRequest {
        use arclain_data::{DataRequest, DataSource, ResourceType};

        let resource_type = match request.resource_type {
            crate::arclain::plugin::host::ResourceType::Binary => ResourceType::Binary,
            crate::arclain::plugin::host::ResourceType::Image => ResourceType::Image,
            crate::arclain::plugin::host::ResourceType::Json => ResourceType::Metadata,
        };

        let mut req = DataRequest::new(&request.key)
            .with_type(resource_type)
            .with_plugin_id(&self.plugin_id);

        tracing::debug!(
            "[HostFunctions::build_data_request] key='{}' plugin_id='{}' url={:?}",
            request.key,
            self.plugin_id,
            request.url,
        );

        if let Some(url) = request.url {
            req = req.with_url(url);
        }
        if let Some(pid) = request.product_id {
            req = req.with_product(pid);
        }

        if !request.sources.is_empty() {
            let mut sources = arclain_data::IndexSet::new();
            for src in request.sources {
                let ds = match src {
                    crate::arclain::plugin::host::DataSource::MetadataCache => {
                        DataSource::MetadataStore
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
            // Default: for metadata type, check MetadataCache first.
            let mut sources = arclain_data::IndexSet::new();
            sources.insert(DataSource::MetadataStore);
            sources.insert(DataSource::ContentCache);
            sources.insert(DataSource::Network);
            req = req.with_sources(sources);
        }

        req
    }

    /// Create a file in the host's temp directory
    pub(super) fn impl_create_file(
        &self,
        filename: String,
        content: Vec<u8>,
    ) -> Result<String, String> {
        use std::io::Write;

        let mut path = std::env::temp_dir();
        // Sanitize filename to prevent path traversal
        let safe_filename = filename.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
        path.push(&safe_filename);

        let mut file =
            std::fs::File::create(&path).map_err(|e| format!("Failed to create file: {}", e))?;
        file.write_all(&content)
            .map_err(|e| format!("Failed to write file: {}", e))?;

        let path_str = path.to_string_lossy().into_owned();
        tracing::debug!("[HostFunctions] Created file: {}", path_str);
        Ok(path_str)
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

    fn rename_archive(&mut self, new_name: String) -> std::result::Result<String, String> {
        self.impl_rename_archive(new_name)
    }

    fn emit_metadata(&mut self, metadata_json: String) {
        self.impl_emit_metadata(metadata_json)
    }

    fn show_message(&mut self, title: String, message: String) {
        self.impl_show_message(title, message)
    }

    fn set_status_message(&mut self, message: String) {
        // Store the status bar message for the UI to pick up
        *self.pending_status_message.lock() = Some(message);
    }

    fn copy_to_clipboard(&mut self, text: String) -> bool {
        self.impl_copy_to_clipboard(text)
    }

    fn list_cached_entries(&mut self) -> Vec<String> {
        self.impl_list_cached_entries()
    }

    fn get_metadata_summaries(
        &mut self,
        ids: Vec<String>,
    ) -> Vec<crate::arclain::plugin::host::MetadataSummary> {
        self.impl_get_metadata_summaries(ids)
    }

    fn get_product_metadata(&mut self, product_id: String, source: String) -> Option<String> {
        self.impl_get_product_metadata(product_id, source)
    }

    fn export_cache(&mut self) -> Result<String, String> {
        self.impl_export_cache()
    }

    fn import_cache(&mut self) -> Result<String, String> {
        self.impl_import_cache()
    }

    // === Data API (unified) ===
    fn request_data(&mut self, request: crate::arclain::plugin::host::DataRequest) -> String {
        let req = self.build_data_request(request);
        self.data_service.request_data(req)
    }

    fn fetch_to_cache(&mut self, request: crate::arclain::plugin::host::DataRequest) -> bool {
        // Build the request the same way `request_data` does, then run
        // it through `data_service.resolve` directly. The Network
        // resolver's `try_resolve` writes the bytes into ContentCache
        // as a side effect (see `DataService::resolve` →
        // `store_to_caches`); we discard the returned `data` so the
        // body never crosses back over the WASM ABI. Used for big
        // blobs (chobit videos, etc.) where the round-trip would burn
        // through plugin heap.
        let req = self.build_data_request(request);
        let key = req.key.clone();
        let result = self.data_service.resolve(&req);
        let ok = matches!(
            result.status,
            arclain_data::DataStatus::Ready | arclain_data::DataStatus::Cached,
        );
        if ok {
            tracing::debug!(
                "[HostFunctions::fetch_to_cache] key='{}' stored, dropping {} bytes from WASM path",
                key,
                result.data.as_ref().map(|d| d.len()).unwrap_or(0),
            );
        } else {
            tracing::warn!(
                "[HostFunctions::fetch_to_cache] key='{}' failed: {}",
                key,
                result.error.as_deref().unwrap_or("unknown"),
            );
        }
        ok
    }

    fn play_cached_blob(
        &mut self,
        key: String,
        extension: String,
    ) -> std::result::Result<(), String> {
        // Pull the blob out of the host's content cache.
        let bytes = self
            .data_service
            .get_data(&key)
            .ok_or_else(|| format!("no cached blob for key '{}'", key))?;

        // OS players (especially on Windows) sniff the file extension
        // to pick a codec — cacache's content-addressable filenames
        // don't have one, so we materialise a copy in temp with the
        // caller-supplied extension. Sanitise both the extension
        // ("mp4" / "webm" / etc.) and the key (it contains colons:
        // `dlsite:RJ123:video:abc:720p`).
        let safe_ext: String = extension
            .trim_start_matches('.')
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(8)
            .collect();
        let ext = if safe_ext.is_empty() {
            "bin".to_string()
        } else {
            safe_ext
        };

        let safe_key = key.replace(
            [':', '/', '\\', '*', '?', '"', '<', '>', '|', ' '],
            "_",
        );

        let mut path = std::env::temp_dir();
        path.push(format!("arclain_{}.{}", safe_key, ext));

        std::fs::write(&path, &bytes).map_err(|e| {
            format!("Failed to write temp blob to {}: {}", path.display(), e)
        })?;

        tracing::info!(
            "[HostFunctions::play_cached_blob] key='{}' -> {} ({} bytes); shelling out",
            key,
            path.display(),
            bytes.len(),
        );

        // `open::that` (v5) returns Ok(()) once the OS shell handler
        // has accepted the request — the launched app then runs
        // asynchronously, so this function returns control even
        // though playback continues.
        open::that(&path).map_err(|e| {
            format!("Failed to launch default app for {}: {}", path.display(), e)
        })?;

        Ok(())
    }

    fn poll_data(&mut self, request_id: String) -> crate::arclain::plugin::host::DataResult {
        tracing::debug!(
            "[HostFunctions::poll_data] Polling for request_id: {}",
            request_id
        );
        let result = self.data_service.poll_data(&request_id);
        tracing::debug!(
            "[HostFunctions::poll_data] Result status: {:?}, has_data: {}, error: {:?}",
            result.status,
            result.data.is_some(),
            result.error
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
        self.data_service.has_data(&key)
    }

    fn get_data(&mut self, key: String) -> Option<Vec<u8>> {
        self.data_service.get_data(&key)
    }

    fn invalidate_cache(&mut self, key: String) -> bool {
        tracing::debug!(
            "[HostFunctions] Cache invalidation requested for key: {}",
            key
        );

        // Check for wildcard pattern
        if key.ends_with('*') {
            tracing::debug!("[HostFunctions] Wildcard pattern detected: {}", key);

            // Delete from content cache using pattern
            let mut count = 0;
            if let Some(cache) = &self.content_cache {
                if let Ok(c) = cache.remove_by_pattern(&key) {
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
            if let Ok(true) = cache.remove(&key) {
                tracing::debug!("[HostFunctions] Invalidated content cache key: {}", key);
                invalidated = true;
            }
        }

        invalidated
    }

    fn create_file(&mut self, filename: String, content: Vec<u8>) -> Result<String, String> {
        self.impl_create_file(filename, content)
    }
}

// Implement the ui::Host trait (empty - ui interface only defines types)
impl crate::arclain::plugin::ui::Host for HostFunctions {}

// Implement the rules::Host trait (empty - rules interface only defines types)
impl crate::arclain::plugin::rules::Host for HostFunctions {}

// Implement the meta::Host trait (empty - meta interface only defines types)
impl crate::arclain::plugin::meta::Host for HostFunctions {}
