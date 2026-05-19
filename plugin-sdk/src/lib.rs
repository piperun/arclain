//! Archust Plugin SDK - WASI Component Model bindings for Arclain plugins

// Generate WIT bindings in a submodule to avoid macro name conflicts
pub mod bindings {
    wit_bindgen::generate!({
        path: "../wit/arclain.wit",
        world: "plugin-world",
        pub_export_macro: true,
    });
}

// Re-export everything from bindings so they are available at crate root
pub use bindings::*;

// Helper logging functions
pub fn info(msg: &str) {
    arclain::plugin::host::log(arclain::plugin::host::LogLevel::Info, msg);
}

pub fn warn(msg: &str) {
    arclain::plugin::host::log(arclain::plugin::host::LogLevel::Warn, msg);
}

pub fn error(msg: &str) {
    arclain::plugin::host::log(arclain::plugin::host::LogLevel::Error, msg);
}

pub fn debug(msg: &str) {
    arclain::plugin::host::log(arclain::plugin::host::LogLevel::Debug, msg);
}

// Archive context helpers
pub fn current_archive_info() -> Option<arclain::plugin::host::ArchiveInfo> {
    arclain::plugin::host::current_archive_info()
}

// Metadata helpers
pub fn emit_metadata(metadata_json: &str) {
    arclain::plugin::host::emit_metadata(metadata_json);
}

// Archive helpers
pub fn list_archive_files() -> Result<Vec<String>, String> {
    arclain::plugin::host::list_archive_files()
}

/// Rename the currently open archive file
/// Takes a new filename (not full path) and returns the new full path on success
/// Requires ArchiveModify capability in the plugin manifest
pub fn rename_archive(new_name: &str) -> Result<String, String> {
    arclain::plugin::host::rename_archive(new_name)
}

// UI helpers
pub fn show_message(title: &str, message: &str) {
    arclain::plugin::host::show_message(title, message);
}

pub fn log_network_activity(msg: &str) {
    arclain::plugin::host::log_network_activity(msg);
}

/// Invalidate a cache entry to force a refetch from network
pub fn invalidate_cache(key: &str) -> bool {
    arclain::plugin::host::invalidate_cache(key)
}

// === Data API Helpers ===

// Only expose what plugins need - NOT cache internals
pub use arclain::plugin::host::{DataRequest, DataResult, DataStatus, ResourceType};

/// Request data from a URL using the Data API
/// The host handles caching transparently.
pub fn request_data(key: &str, url: &str, resource_type: ResourceType) -> String {
    let req = DataRequest {
        key: key.to_string(),
        url: Some(url.to_string()),
        resource_type,
        product_id: None,
        sources: vec![], // Host decides the best sources
    };
    arclain::plugin::host::request_data(&req)
}

/// Poll for the status of a request
pub fn poll_data(request_id: &str) -> DataResult {
    arclain::plugin::host::poll_data(request_id)
}

/// Interval between `poll_data` calls when waiting for an in-flight
/// request. The host currently resolves synchronously so a `Pending`
/// or `Fetching` status is rare, but we keep a small backoff in case
/// that changes (audit: magic-number callout).
const FETCH_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// Fetch data blocking until complete
/// Host automatically checks cache before network.
pub fn fetch_blocking(
    key: &str,
    url: &str,
    resource_type: ResourceType,
) -> Result<Vec<u8>, String> {
    let req_id = request_data(key, url, resource_type);

    // Debug: Log the request ID
    info(&format!(
        "[SDK] fetch_blocking: key={}, req_id={}",
        key, req_id
    ));

    loop {
        let result = poll_data(&req_id);

        // Debug: Log what we received from host
        info(&format!(
            "[SDK] poll_data result: status={:?}, has_data={}, error={:?}",
            result.status,
            result.data.is_some(),
            result.error
        ));

        match result.status {
            DataStatus::Ready | DataStatus::Cached => {
                if let Some(ref data) = result.data {
                    let preview: Vec<u8> = data.iter().take(20).copied().collect();
                    info(&format!(
                        "[SDK] Data bytes (first 20): {:?}, len={}",
                        preview,
                        data.len()
                    ));
                }
                return result.data.ok_or_else(|| "No data returned".to_string());
            }
            DataStatus::Failed => {
                return Err(result.error.unwrap_or_else(|| "Unknown error".to_string()));
            }
            DataStatus::Pending | DataStatus::Fetching => {
                std::thread::sleep(FETCH_POLL_INTERVAL);
            }
        }
    }
}

/// String fetch helper (utf8 decode)
pub fn fetch_string_blocking(key: &str, url: &str) -> Result<String, String> {
    let bytes = fetch_blocking(key, url, ResourceType::Json)?;
    String::from_utf8(bytes).map_err(|e| format!("UTF8 error: {}", e))
}

/// Fetch `url` and store it in the host's content cache under `key`,
/// **without** moving the body across the WASM ABI. Returns `Ok(())`
/// on success; the bytes never enter the plugin's heap.
///
/// Use this for big blobs — videos, archive blobs — where
/// `fetch_blocking` would force the full body through three buffers
/// (host vec → ABI copy → plugin vec). Subsequent
/// `arclain::plugin::host::has_data(key)` /
/// `arclain::plugin::host::get_data(key)` see the cached entry the
/// same way they would after a `fetch_blocking`.
pub fn fetch_to_cache(key: &str, url: &str, resource_type: ResourceType) -> Result<(), String> {
    let req = DataRequest {
        key: key.to_string(),
        url: Some(url.to_string()),
        resource_type,
        product_id: None,
        sources: vec![],
    };
    if arclain::plugin::host::fetch_to_cache(&req) {
        Ok(())
    } else {
        Err(format!("fetch_to_cache failed for {}", key))
    }
}

/// Hand a cached blob to the OS default application. The host writes
/// the bytes to a temp file with `extension` (no leading dot — e.g.
/// `"mp4"`, `"webm"`) and shells out via the system file-association
/// handler (mpv / VLC / whatever the user has registered).
///
/// The bytes never re-enter the WASM sandbox; the plugin only needs
/// to know the cache key. Returns `Err` if the cache entry is
/// missing, the temp write fails, or the OS launcher fails.
pub fn play_cached_blob(key: &str, extension: &str) -> Result<(), String> {
    arclain::plugin::host::play_cached_blob(key, extension)
}

// Cache helpers (for cache management UI, not data access)
pub fn list_cached_entries() -> Result<Vec<String>, String> {
    Ok(arclain::plugin::host::list_cached_entries())
}

/// Re-export MetadataSummary for use in plugins
pub use arclain::plugin::host::MetadataSummary;

/// Batch query for metadata summaries (id, title, geo_blocked)
/// Much faster than individual lookups for list rendering
pub fn get_metadata_summaries(ids: Vec<String>) -> Vec<MetadataSummary> {
    arclain::plugin::host::get_metadata_summaries(&ids)
}

/// Get full product metadata from database with fallback chain:
/// 1. metadata.sqlite (instant - already parsed)
/// 2. JSON cache (host parses + saves to DB)
/// 3. HTML cache (host parses + saves to DB)
/// Returns ProductMetadata as JSON string, ready to deserialize.
/// This is the preferred way to get metadata - no WASM-side parsing needed.
pub fn get_product_metadata(product_id: &str, source: &str) -> Option<String> {
    arclain::plugin::host::get_product_metadata(product_id, source)
}

/// Create a file in the host's temp directory
/// Returns the full path to the created file
pub fn create_file(filename: &str, content: &[u8]) -> Result<String, String> {
    arclain::plugin::host::create_file(filename, content)
}
