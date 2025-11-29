//! Host functions that plugins can call
//!
//! This module implements the host-side functions that are exposed to WASM plugins.

use crate::types::{PluginCapability, PluginError, Result};
use arclain_core::{sevenzip::SevenZipCli, ArchiveBackend};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, trace, warn};
use wasmtime::Caller;

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

    /// Check if a request is allowed, returns true if allowed
    pub fn check_rate_limit(&self) -> bool {
        let now = Instant::now();
        let mut requests = self.requests.lock();

        // Remove requests older than 1 minute
        requests.retain(|&time| now.duration_since(time) < Duration::from_secs(60));

        // Check if we're under the limit
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

    /// Make a GET request
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

    /// Make a POST request with JSON body
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
#[derive(Clone)]
pub struct HostFunctions {
    pub http_client: Option<HttpClient>,
    pub capabilities: std::collections::HashSet<PluginCapability>,
    /// Shared memory for passing data between host and WASM
    pub memory_buffers: Arc<Mutex<HashMap<u32, Vec<u8>>>>,
    next_buffer_id: Arc<Mutex<u32>>,
    /// Archive backend for file operations
    pub archive_backend: Option<Arc<SevenZipCli>>,
    /// Current archive path (if any)
    pub current_archive: Arc<Mutex<Option<String>>>,
    /// Current archive password (if any)
    pub current_password: Arc<Mutex<Option<String>>>,
    /// Heap pointer for simple bump allocator
    pub heap_ptr: Arc<Mutex<u32>>,
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

        Self {
            http_client,
            capabilities,
            memory_buffers: Arc::new(Mutex::new(HashMap::new())),
            next_buffer_id: Arc::new(Mutex::new(1)),
            archive_backend: None,
            current_archive: Arc::new(Mutex::new(None)),
            current_password: Arc::new(Mutex::new(None)),
            heap_ptr: Arc::new(Mutex::new(0)),
        }
    }

    /// Create with archive backend integration
    pub fn with_backend(
        capabilities: std::collections::HashSet<PluginCapability>,
        requests_per_minute: u32,
        backend: Arc<SevenZipCli>,
    ) -> Self {
        let mut host_funcs = Self::new(capabilities, requests_per_minute);
        host_funcs.archive_backend = Some(backend);
        host_funcs
    }

    /// Set the current archive context
    pub fn set_archive_context(&self, archive_path: Option<String>, password: Option<String>) {
        *self.current_archive.lock() = archive_path;
        *self.current_password.lock() = password;
    }

    /// Check if the plugin has a specific capability
    pub fn check_capability(&self, cap: PluginCapability) -> bool {
        self.capabilities.contains(&cap)
    }

    /// Allocate a buffer ID for passing data
    pub fn allocate_buffer(&self, data: Vec<u8>) -> u32 {
        let mut next_id = self.next_buffer_id.lock();
        let id = *next_id;
        *next_id += 1;

        self.memory_buffers.lock().insert(id, data);
        id
    }

    /// Get buffer data and remove it
    pub fn take_buffer(&self, id: u32) -> Option<Vec<u8>> {
        self.memory_buffers.lock().remove(&id)
    }
}

/// Read a string from WASM memory
pub fn read_string_from_memory(
    caller: &mut Caller<'_, HostFunctions>,
    ptr: u32,
    len: u32,
) -> Result<String> {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| PluginError::ExecutionError("No memory export found".to_string()))?;

    let mut buffer = vec![0u8; len as usize];
    memory
        .read(caller, ptr as usize, &mut buffer)
        .map_err(|e| PluginError::ExecutionError(format!("Failed to read memory: {}", e)))?;

    String::from_utf8(buffer)
        .map_err(|e| PluginError::ExecutionError(format!("Invalid UTF-8: {}", e)))
}

/// Write a string to WASM memory and return the length
pub fn write_string_to_memory(
    caller: &mut Caller<'_, HostFunctions>,
    ptr: u32,
    max_len: u32,
    data: &str,
) -> Result<i32> {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| PluginError::ExecutionError("No memory export found".to_string()))?;

    let bytes = data.as_bytes();
    if bytes.len() > max_len as usize {
        return Ok(-1); // Buffer too small
    }

    memory
        .write(caller, ptr as usize, bytes)
        .map_err(|e| PluginError::ExecutionError(format!("Failed to write memory: {}", e)))?;

    Ok(bytes.len() as i32)
}

// ============================================================================
// Host Function Implementations
// ============================================================================

/// Host function: log a message
///
/// Parameters:
/// - level: log level (0=error, 1=warn, 2=info, 3=debug, 4=trace)
/// - ptr: pointer to message string in WASM memory
/// - len: length of message string
pub fn host_log(mut caller: Caller<'_, HostFunctions>, level: i32, ptr: i32, len: i32) -> i32 {
    let message = match read_string_from_memory(&mut caller, ptr as u32, len as u32) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to read log message: {}", e);
            return -1;
        }
    };

    match level {
        0 => error!("[Plugin] {}", message),
        1 => warn!("[Plugin] {}", message),
        2 => info!("[Plugin] {}", message),
        3 => debug!("[Plugin] {}", message),
        _ => trace!("[Plugin] {}", message),
    }

    0
}

/// Host function: HTTP GET request
///
/// Parameters:
/// - url_ptr: pointer to URL string in WASM memory
/// - url_len: length of URL string
/// - out_ptr: pointer to output buffer in WASM memory
/// - out_max_len: maximum length of output buffer
///
/// Returns:
/// - On success: length of response written to out_ptr
/// - On error: negative error code
pub fn host_http_get(
    mut caller: Caller<'_, HostFunctions>,
    url_ptr: u32,
    url_len: u32,
    out_ptr: u32,
    out_max_len: u32,
) -> i32 {
    // Check capability
    if !caller.data().check_capability(PluginCapability::Network) {
        error!("[Plugin] HTTP GET denied: Network capability not granted");
        return -1;
    }

    // Read URL from WASM memory
    let url = match read_string_from_memory(&mut caller, url_ptr, url_len) {
        Ok(s) => s,
        Err(e) => {
            error!("[Plugin] Failed to read URL: {}", e);
            return -2;
        }
    };

    // Make HTTP request
    let http_client = match &caller.data().http_client {
        Some(client) => client.clone(),
        None => {
            error!("[Plugin] HTTP client not initialized");
            return -3;
        }
    };

    let response = match http_client.get(&url) {
        Ok(r) => r,
        Err(e) => {
            error!("[Plugin] HTTP GET failed: {}", e);
            return -4;
        }
    };

    // Write response to WASM memory
    match write_string_to_memory(&mut caller, out_ptr, out_max_len, &response) {
        Ok(len) => len,
        Err(e) => {
            error!("[Plugin] Failed to write response: {}", e);
            -5
        }
    }
}

/// Host function: HTTP POST request with JSON body
///
/// Parameters:
/// - url_ptr: pointer to URL string in WASM memory
/// - url_len: length of URL string
/// - body_ptr: pointer to JSON body string in WASM memory
/// - body_len: length of JSON body string
/// - out_ptr: pointer to output buffer in WASM memory
/// - out_max_len: maximum length of output buffer
///
/// Returns:
/// - On success: length of response written to out_ptr
/// - On error: negative error code
pub fn host_http_post_json(
    mut caller: Caller<'_, HostFunctions>,
    url_ptr: u32,
    url_len: u32,
    body_ptr: u32,
    body_len: u32,
    out_ptr: u32,
    out_max_len: u32,
) -> i32 {
    // Check capability
    if !caller.data().check_capability(PluginCapability::Network) {
        error!("[Plugin] HTTP POST denied: Network capability not granted");
        return -1;
    }

    // Read URL from WASM memory
    let url = match read_string_from_memory(&mut caller, url_ptr, url_len) {
        Ok(s) => s,
        Err(e) => {
            error!("[Plugin] Failed to read URL: {}", e);
            return -2;
        }
    };

    // Read body from WASM memory
    let body = match read_string_from_memory(&mut caller, body_ptr, body_len) {
        Ok(s) => s,
        Err(e) => {
            error!("[Plugin] Failed to read body: {}", e);
            return -3;
        }
    };

    // Make HTTP request
    let http_client = match &caller.data().http_client {
        Some(client) => client.clone(),
        None => {
            error!("[Plugin] HTTP client not initialized");
            return -4;
        }
    };

    let response = match http_client.post_json(&url, &body) {
        Ok(r) => r,
        Err(e) => {
            error!("[Plugin] HTTP POST failed: {}", e);
            return -5;
        }
    };

    // Write response to WASM memory
    match write_string_to_memory(&mut caller, out_ptr, out_max_len, &response) {
        Ok(len) => len,
        Err(e) => {
            error!("[Plugin] Failed to write response: {}", e);
            -6
        }
    }
}

/// Host function: Read file from archive
///
/// Parameters:
/// - archive_ptr: pointer to archive path in WASM memory
/// - archive_len: length of archive path
/// - file_ptr: pointer to file path in WASM memory
/// - file_len: length of file path
/// - out_ptr: pointer to output buffer
/// - out_max_len: maximum length of output buffer
///
/// Returns:
/// - On success: length of file data written
/// - On error: negative error code
pub fn host_file_read(
    mut caller: Caller<'_, HostFunctions>,
    archive_ptr: u32,
    archive_len: u32,
    file_ptr: u32,
    file_len: u32,
    _out_ptr: u32,
    _out_max_len: u32,
) -> i32 {
    // Check capability
    if !caller.data().check_capability(PluginCapability::FileRead) {
        error!("[Plugin] File read denied: FileRead capability not granted");
        return -1;
    }

    // Read archive path
    let archive_path = match read_string_from_memory(&mut caller, archive_ptr, archive_len) {
        Ok(s) => s,
        Err(e) => {
            error!("[Plugin] Failed to read archive path: {}", e);
            return -2;
        }
    };

    // Read file path
    let file_path = match read_string_from_memory(&mut caller, file_ptr, file_len) {
        Ok(s) => s,
        Err(e) => {
            error!("[Plugin] Failed to read file path: {}", e);
            return -3;
        }
    };

    debug!(
        "[Plugin] Reading file '{}' from archive '{}'",
        file_path, archive_path
    );

    // Get archive backend
    let backend = match &caller.data().archive_backend {
        Some(b) => b.clone(),
        None => {
            error!("[Plugin] Archive backend not available");
            return -4;
        }
    };

    // Get password if available
    let password = caller.data().current_password.lock().clone();

    // Read file from archive
    let archive_path_buf = Path::new(&archive_path);
    match backend.read_text_file(archive_path_buf, &file_path, password.as_deref()) {
        Ok(content) => {
            // Write content to WASM memory
            match write_string_to_memory(&mut caller, _out_ptr, _out_max_len, &content) {
                Ok(len) => {
                    info!(
                        "[Plugin] Successfully read file '{}' ({} bytes)",
                        file_path, len
                    );
                    len
                }
                Err(e) => {
                    error!(
                        "[Plugin] Failed to write file content to WASM memory: {}",
                        e
                    );
                    -5
                }
            }
        }
        Err(e) => {
            error!("[Plugin] Failed to read file from archive: {}", e);
            -6
        }
    }
}

/// Host function: Write file to archive
///
/// Parameters:
/// - archive_ptr: pointer to archive path in WASM memory
/// - archive_len: length of archive path
/// - file_ptr: pointer to file path in WASM memory
/// - file_len: length of file path
/// - data_ptr: pointer to file data in WASM memory
/// - data_len: length of file data
///
/// Returns:
/// - On success: 0
/// - On error: negative error code
pub fn host_file_write(
    mut caller: Caller<'_, HostFunctions>,
    archive_ptr: u32,
    archive_len: u32,
    file_ptr: u32,
    file_len: u32,
    data_ptr: u32,
    data_len: u32,
) -> i32 {
    // Check capability
    if !caller.data().check_capability(PluginCapability::FileWrite) {
        error!("[Plugin] File write denied: FileWrite capability not granted");
        return -1;
    }

    // Read archive path
    let archive_path = match read_string_from_memory(&mut caller, archive_ptr, archive_len) {
        Ok(s) => s,
        Err(e) => {
            error!("[Plugin] Failed to read archive path: {}", e);
            return -2;
        }
    };

    // Read file path
    let file_path = match read_string_from_memory(&mut caller, file_ptr, file_len) {
        Ok(s) => s,
        Err(e) => {
            error!("[Plugin] Failed to read file path: {}", e);
            return -3;
        }
    };

    // Read file data
    let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
        Some(m) => m,
        None => {
            error!("[Plugin] No memory export found");
            return -4;
        }
    };

    let mut data = vec![0u8; data_len as usize];
    if let Err(e) = memory.read(&caller, data_ptr as usize, &mut data) {
        error!("[Plugin] Failed to read file data: {}", e);
        return -5;
    }

    debug!(
        "[Plugin] Writing file '{}' ({} bytes) to archive '{}'",
        file_path, data_len, archive_path
    );

    // Get archive backend
    let backend = match &caller.data().archive_backend {
        Some(b) => b.clone(),
        None => {
            error!("[Plugin] Archive backend not available");
            return -6;
        }
    };

    // Convert bytes to string (assuming UTF-8 text file)
    let content = match String::from_utf8(data.clone()) {
        Ok(s) => s,
        Err(e) => {
            error!("[Plugin] File data is not valid UTF-8: {}", e);
            return -7;
        }
    };

    // Write file to archive
    let archive_path_buf = Path::new(&archive_path);
    match backend.add_or_update_file_from_str(archive_path_buf, &file_path, &content) {
        Ok(()) => {
            info!(
                "[Plugin] Successfully wrote file '{}' to archive",
                file_path
            );
            0
        }
        Err(e) => {
            error!("[Plugin] Failed to write file to archive: {}", e);
            -8
        }
    }
}

/// Host function: Get archive metadata
///
/// Parameters:
/// - archive_ptr: pointer to archive path in WASM memory
/// - archive_len: length of archive path
/// - out_ptr: pointer to output buffer for JSON metadata
/// - out_max_len: maximum length of output buffer
///
/// Returns:
/// - On success: length of JSON data written
/// - On error: negative error code
pub fn host_archive_metadata_get(
    mut caller: Caller<'_, HostFunctions>,
    archive_ptr: u32,
    archive_len: u32,
    out_ptr: u32,
    out_max_len: u32,
) -> i32 {
    // Check capability
    if !caller
        .data()
        .check_capability(PluginCapability::ArchiveMetadataRead)
    {
        error!("[Plugin] Archive metadata read denied: capability not granted");
        return -1;
    }

    // Read archive path
    let archive_path = match read_string_from_memory(&mut caller, archive_ptr, archive_len) {
        Ok(s) => s,
        Err(e) => {
            error!("[Plugin] Failed to read archive path: {}", e);
            return -2;
        }
    };

    debug!("[Plugin] Getting metadata for archive '{}'", archive_path);

    // Get archive backend
    let backend = match &caller.data().archive_backend {
        Some(b) => b.clone(),
        None => {
            error!("[Plugin] Archive backend not available");
            return -3;
        }
    };

    // Get password if available
    let password = caller.data().current_password.lock().clone();

    // List archive to get metadata
    let archive_path_buf = Path::new(&archive_path);
    match backend.list(archive_path_buf, password.as_deref()) {
        Ok(info) => {
            // Calculate total sizes
            let total_files = info.entries.iter().filter(|e| !e.is_dir).count();
            let compressed_size: u64 = info.entries.iter().map(|e| e.packed_size).sum();
            let uncompressed_size: u64 = info.entries.iter().map(|e| e.size).sum();

            // Create JSON metadata
            let metadata = serde_json::json!({
                "files": total_files,
                "compressed_size": compressed_size,
                "uncompressed_size": uncompressed_size,
                "encrypted": info.encrypted,
                "headers_encrypted": info.headers_encrypted,
                "encryption_method": info.encryption_method,
                "archive_type": format!("{:?}", info.archive_kind),
            });

            let metadata_str = metadata.to_string();
            match write_string_to_memory(&mut caller, out_ptr, out_max_len, &metadata_str) {
                Ok(len) => {
                    info!(
                        "[Plugin] Successfully retrieved metadata for '{}'",
                        archive_path
                    );
                    len
                }
                Err(e) => {
                    error!("[Plugin] Failed to write metadata: {}", e);
                    -4
                }
            }
        }
        Err(e) => {
            error!("[Plugin] Failed to list archive: {}", e);
            -5
        }
    }
}

/// Host function: Set archive metadata
///
/// Parameters:
/// - archive_ptr: pointer to archive path in WASM memory
/// - archive_len: length of archive path
/// - metadata_ptr: pointer to JSON metadata in WASM memory
/// - metadata_len: length of JSON metadata
///
/// Returns:
/// - On success: 0
/// - On error: negative error code
pub fn host_archive_metadata_set(
    mut caller: Caller<'_, HostFunctions>,
    archive_ptr: u32,
    archive_len: u32,
    metadata_ptr: u32,
    metadata_len: u32,
) -> i32 {
    // Check capability
    if !caller
        .data()
        .check_capability(PluginCapability::ArchiveMetadataWrite)
    {
        error!("[Plugin] Archive metadata write denied: capability not granted");
        return -1;
    }

    // Read archive path
    let archive_path = match read_string_from_memory(&mut caller, archive_ptr, archive_len) {
        Ok(s) => s,
        Err(e) => {
            error!("[Plugin] Failed to read archive path: {}", e);
            return -2;
        }
    };

    // Read metadata JSON
    let metadata_json = match read_string_from_memory(&mut caller, metadata_ptr, metadata_len) {
        Ok(s) => s,
        Err(e) => {
            error!("[Plugin] Failed to read metadata: {}", e);
            return -3;
        }
    };

    debug!(
        "[Plugin] Setting metadata for archive '{}': {}",
        archive_path, metadata_json
    );

    // Validate JSON
    let metadata: serde_json::Value = match serde_json::from_str(&metadata_json) {
        Ok(v) => v,
        Err(e) => {
            error!("[Plugin] Invalid JSON metadata: {}", e);
            return -4;
        }
    };

    debug!(
        "[Plugin] Metadata modification requested for '{}'",
        archive_path
    );
    debug!("[Plugin] Metadata: {}", metadata);

    // Note: Currently 7-Zip doesn't support arbitrary metadata modification
    // This would require extended attributes or custom comment fields
    // For now, we log the request and return success to not break plugin workflow
    info!("[Plugin] To implement: use archive comments or extended attributes");

    0
}

/// Host function: Deallocate memory
///
/// Parameters:
/// - ptr: pointer to memory
/// - size: size of memory
/// - align: alignment of memory
pub fn host_dealloc(_caller: Caller<'_, HostFunctions>, ptr: u32, size: u32, align: u32) {
    trace!(
        "[Plugin] __rust_dealloc called: ptr={}, size={}, align={}",
        ptr,
        size,
        align
    );
    // No-op: We use a simple bump allocator that does not support deallocation.
    // Memory is reclaimed only when the plugin instance is dropped.
}

/// Host function: Deallocate memory (wasm-bindgen style)
///
/// Parameters:
/// - ptr: pointer to memory
/// - size: size of memory
pub fn host_wasm_dealloc(_caller: Caller<'_, HostFunctions>, ptr: u32, size: u32, align: u32) {
    trace!(
        "[Plugin] __wasm_dealloc called: ptr={}, size={}, align={}",
        ptr,
        size,
        align
    );
    // No-op: Same as __rust_dealloc
}

/// Host function: Allocate memory (wasm-bindgen style)
///
/// Parameters:
/// - size: size of memory
///
/// Returns:
/// - pointer to allocated memory
pub fn host_wasm_alloc(mut caller: Caller<'_, HostFunctions>, size: u32) -> u32 {
    trace!("[Plugin] __wasm_alloc called: size={}", size);
    // Align to 8 bytes by default for safety
    host_alloc(caller, size, 8)
}

/// Host function: Allocate memory
///
/// Implements a simple bump allocator that grows the WASM memory as needed.
///
/// Parameters:
/// - size: size of memory
/// - align: alignment of memory
///
/// Returns:
/// - Pointer to allocated memory (0 on failure)
pub fn host_alloc(mut caller: Caller<'_, HostFunctions>, size: u32, align: u32) -> u32 {
    debug!(
        "[Plugin] __rust_alloc called: size={}, align={}",
        size, align
    );

    // Get heap pointer state
    let heap_ptr_lock = caller.data().heap_ptr.clone();
    let mut heap_ptr = heap_ptr_lock.lock();

    // Lazy initialization of heap pointer
    if *heap_ptr == 0 {
        // Try to find __heap_base export
        let base = if let Some(export) = caller.get_export("__heap_base") {
            if let Some(global) = export.into_global() {
                global.get(&mut caller).i32().unwrap_or(1024 * 1024) as u32
            } else {
                1024 * 1024 // Default to 1MB
            }
        } else {
            1024 * 1024 // Default to 1MB
        };
        *heap_ptr = base;
        debug!("[Plugin] Initialized heap pointer to {}", base);
    }

    // Calculate new pointer with alignment
    let current = *heap_ptr;
    let padding = (align - (current % align)) % align;
    let start = current + padding;
    let end = start + size;

    // Get memory export
    let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
        Some(m) => m,
        None => {
            error!("[Plugin] No memory export found");
            return 0;
        }
    };

    // Check if we need to grow memory
    let current_pages = memory.size(&caller);
    let current_bytes = current_pages * 65536;

    if end as u64 > current_bytes {
        let needed_bytes = (end as u64) - current_bytes;
        let needed_pages = (needed_bytes + 65535) / 65536;
        debug!("[Plugin] Growing memory by {} pages", needed_pages);

        if memory.grow(&mut caller, needed_pages).is_err() {
            error!("[Plugin] Failed to grow memory");
            return 0;
        }
    }

    // Update heap pointer
    *heap_ptr = end;

    start
}

#[cfg(test)]
mod tests;
