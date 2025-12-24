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

/// Fetch data blocking until complete
/// Host automatically checks cache before network.
pub fn fetch_blocking(
    key: &str,
    url: &str,
    resource_type: ResourceType,
) -> Result<Vec<u8>, String> {
    let req_id = request_data(key, url, resource_type);

    loop {
        let result = poll_data(&req_id);
        match result.status {
            DataStatus::Ready | DataStatus::Cached => {
                return result.data.ok_or_else(|| "No data returned".to_string());
            }
            DataStatus::Failed => {
                return Err(result.error.unwrap_or_else(|| "Unknown error".to_string()));
            }
            DataStatus::Pending | DataStatus::Fetching => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

/// String fetch helper (utf8 decode)
pub fn fetch_string_blocking(key: &str, url: &str) -> Result<String, String> {
    let bytes = fetch_blocking(key, url, ResourceType::Json)?;
    String::from_utf8(bytes).map_err(|e| format!("UTF8 error: {}", e))
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
