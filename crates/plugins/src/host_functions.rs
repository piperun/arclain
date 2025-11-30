//! Host functions that plugins can call
//!
//! This module implements the host-side functions that are exposed to WASM plugins
//! via the WASI Component Model.

use crate::arclain::plugin::host::{Host, LogLevel};
use crate::types::{PluginCapability, PluginError, Result};
use arclain_core::{sevenzip::SevenZipCli, ArchiveBackend};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, trace, warn};
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};

/// HTTP request rate limiter
#[derive(Debug, Clone)]
pub struct RateLimiter {
    requests_per_minute: u32,
    requests: Arc<Mutex<Vec<Instant>>>,
}

impl RateLimiter {
    pub fn new(requests_per_minute: u32) -> Self {
        Self {
            requests_per_minute,
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn check_rate_limit(&self) -> bool {
        let now = Instant::now();
        let mut requests = self.requests.lock();
        requests.retain(|&time| now.duration_since(time) < Duration::from_secs(60));

        if requests.len() < self.requests_per_minute as usize {
            requests.push(now);
            true
        } else {
            false
        }
    }
}

/// HTTP client for making requests
#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::blocking::Client,
    rate_limiter: RateLimiter,
}

impl HttpClient {
    pub fn new(requests_per_minute: u32) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Archust-Plugin/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            rate_limiter: RateLimiter::new(requests_per_minute),
        }
    }

    pub fn get(&self, url: &str) -> Result<String> {
        if !self.rate_limiter.check_rate_limit() {
            return Err(PluginError::ExecutionError(
                "Rate limit exceeded".to_string(),
            ));
        }
        debug!("HTTP GET: {}", url);
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|e| PluginError::ExecutionError(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(PluginError::ExecutionError(format!(
                "HTTP error: {}",
                response.status()
            )));
        }
        response
            .text()
            .map_err(|e| PluginError::ExecutionError(format!("Failed to read response: {}", e)))
    }

    pub fn post_json(&self, url: &str, body: &str) -> Result<String> {
        if !self.rate_limiter.check_rate_limit() {
            return Err(PluginError::ExecutionError(
                "Rate limit exceeded".to_string(),
            ));
        }
        debug!("HTTP POST: {}", url);
        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .map_err(|e| PluginError::ExecutionError(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(PluginError::ExecutionError(format!(
                "HTTP error: {}",
                response.status()
            )));
        }
        response
            .text()
            .map_err(|e| PluginError::ExecutionError(format!("Failed to read response: {}", e)))
    }
}

/// State for host functions
pub struct HostFunctions {
    pub http_client: Option<HttpClient>,
    pub capabilities: std::collections::HashSet<PluginCapability>,
    pub archive_backend: Option<Arc<SevenZipCli>>,
    pub current_archive: Arc<Mutex<Option<String>>>,
    pub current_password: Arc<Mutex<Option<String>>>,
    pub settings: Arc<Mutex<HashMap<String, String>>>,
    pub table: ResourceTable,
    pub ctx: WasiCtx,
}

impl HostFunctions {
    pub fn new(
        capabilities: std::collections::HashSet<PluginCapability>,
        requests_per_minute: u32,
    ) -> Self {
        let http_client = if capabilities.contains(&PluginCapability::Network) {
            Some(HttpClient::new(requests_per_minute))
        } else {
            None
        };

        // Initialize WASI context
        let ctx = WasiCtxBuilder::new().inherit_stdio().inherit_args().build();

        Self {
            http_client,
            capabilities,
            archive_backend: None,
            current_archive: Arc::new(Mutex::new(None)),
            current_password: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(HashMap::new())),
            table: ResourceTable::new(),
            ctx,
        }
    }

    pub fn with_backend(
        capabilities: std::collections::HashSet<PluginCapability>,
        requests_per_minute: u32,
        backend: Arc<SevenZipCli>,
    ) -> Self {
        let mut host_funcs = Self::new(capabilities, requests_per_minute);
        host_funcs.archive_backend = Some(backend);
        host_funcs
    }

    pub fn set_archive_context(&self, archive_path: Option<String>, password: Option<String>) {
        *self.current_archive.lock() = archive_path;
        *self.current_password.lock() = password;
    }

    pub fn check_capability(&self, cap: PluginCapability) -> bool {
        self.capabilities.contains(&cap)
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

// Implement the Host trait generated by wit-bindgen
impl Host for HostFunctions {
    fn log(&mut self, level: LogLevel, message: String) {
        match level {
            LogLevel::Error => error!("[Plugin] {}", message),
            LogLevel::Warn => warn!("[Plugin] {}", message),
            LogLevel::Info => info!("[Plugin] {}", message),
            LogLevel::Debug => debug!("[Plugin] {}", message),
            LogLevel::Trace => trace!("[Plugin] {}", message),
        }
    }

    fn get_setting(&mut self, key: String) -> Option<String> {
        self.settings.lock().get(&key).cloned()
    }

    fn set_setting(&mut self, key: String, value: String) {
        self.settings.lock().insert(key, value);
    }

    fn http_get(&mut self, url: String) -> std::result::Result<String, String> {
        if !self.check_capability(PluginCapability::Network) {
            return Err("Network capability not granted".to_string());
        }
        let client = self
            .http_client
            .as_ref()
            .ok_or("HTTP client not initialized")?;
        client.get(&url).map_err(|e| e.to_string())
    }

    fn http_post(&mut self, url: String, body: String) -> std::result::Result<String, String> {
        if !self.check_capability(PluginCapability::Network) {
            return Err("Network capability not granted".to_string());
        }
        let client = self
            .http_client
            .as_ref()
            .ok_or("HTTP client not initialized")?;
        client.post_json(&url, &body).map_err(|e| e.to_string())
    }

    fn file_read(&mut self, archive: String, file: String) -> std::result::Result<String, String> {
        if !self.check_capability(PluginCapability::FileRead) {
            return Err("FileRead capability not granted".to_string());
        }
        let backend = self
            .archive_backend
            .as_ref()
            .ok_or("Archive backend not available")?;
        let password = self.current_password.lock().clone();

        backend
            .read_text_file(Path::new(&archive), &file, password.as_deref())
            .map_err(|e| e.to_string())
    }

    fn file_write(
        &mut self,
        archive: String,
        file: String,
        data: String,
    ) -> std::result::Result<String, String> {
        if !self.check_capability(PluginCapability::FileWrite) {
            return Err("FileWrite capability not granted".to_string());
        }
        let backend = self
            .archive_backend
            .as_ref()
            .ok_or("Archive backend not available")?;

        backend
            .add_or_update_file_from_str(Path::new(&archive), &file, &data)
            .map_err(|e| e.to_string())?;
        Ok("Success".to_string())
    }

    fn current_archive_info(&mut self) -> Option<crate::arclain::plugin::host::ArchiveInfo> {
        // Return current archive path if available
        let archive = self.current_archive.lock().clone()?;
        let path_buf = std::path::PathBuf::from(&archive);
        let filename = path_buf.file_name()?.to_str()?.to_string();

        Some(crate::arclain::plugin::host::ArchiveInfo {
            path: archive,
            filename,
        })
    }

    fn emit_metadata(&mut self, metadata_json: String) {
        // Store metadata for the host to process
        info!("[Plugin] Emitting metadata");
        debug!("[Plugin] Metadata JSON: {}", metadata_json);

        // TODO: Store metadata in a channel/queue for the UI to consume
        // For now, just log it as proof of concept
    }
}

// Implement the ui::Host trait (empty - ui interface only defines types)
impl crate::arclain::plugin::ui::Host for HostFunctions {}
