//! Archust Plugin SDK - WASI Component Model bindings for Wirt plugins

// Generate WIT bindings in a submodule to avoid macro name conflicts
pub mod bindings {
    wit_bindgen::generate!({
        path: "wit/plugin.wit",
        world: "plugin-world",
        pub_export_macro: true,
    });
}

// Re-export everything from bindings so they are available at crate root
pub use bindings::*;

// Helper logging functions. All levels are subject to host entry/rate/daily
// byte caps; admitted Warn/Error messages also reach application tracing.
pub fn info(msg: &str) {
    wirt::plugin::host::log(wirt::plugin::host::LogLevel::Info, msg);
}

pub fn warn(msg: &str) {
    wirt::plugin::host::log(wirt::plugin::host::LogLevel::Warn, msg);
}

pub fn error(msg: &str) {
    wirt::plugin::host::log(wirt::plugin::host::LogLevel::Error, msg);
}

pub fn debug(msg: &str) {
    wirt::plugin::host::log(wirt::plugin::host::LogLevel::Debug, msg);
}

// Archive context helpers (`archive_metadata_read` required)
pub fn current_archive_info() -> Option<wirt::plugin::host::ArchiveInfo> {
    wirt::plugin::host::current_archive_info()
}

/// Publish metadata under `archive_metadata_write` using the payload's source.
/// Prefer [`emit_metadata_for_source`] for new plugins.
pub fn emit_metadata(metadata_json: &str) {
    wirt::plugin::host::emit_metadata(metadata_json);
}

/// Publish metadata for an explicit source.
///
/// The host rejects mismatched payload sources, oversized input, or writes
/// beyond its per-plugin rate, distinct-ID, and session-byte quotas.
pub fn emit_metadata_for_source(source: &str, metadata_json: &str) -> bool {
    wirt::plugin::host::emit_metadata_for_source(source, metadata_json)
}

// Archive helpers (`archive_metadata_read` required for listing). The legacy
// helper returns only the first bounded page.
pub fn list_archive_files() -> Result<Vec<String>, String> {
    wirt::plugin::host::list_archive_files()
}

pub fn archive_file_count() -> Result<u64, String> {
    wirt::plugin::host::archive_file_count()
}

/// List at most 256 archive paths and 1 MiB of path text.
pub fn list_archive_files_page(offset: u32, limit: u32) -> Result<Vec<String>, String> {
    wirt::plugin::host::list_archive_files_page(offset, limit)
}

/// Rename the currently open archive file
/// Takes a new filename (not full path) and returns the new full path on success
/// Requires ArchiveModify capability in the plugin manifest
pub fn rename_archive(new_name: &str) -> Result<String, String> {
    wirt::plugin::host::rename_archive(new_name)
}

/// Deprecated compatibility helper that writes an admitted message only to
/// the bounded plugin log. It does not open a dialog or retain UI state.
#[deprecated(note = "use returned UI actions or bounded plugin logging")]
pub fn show_message(title: &str, message: &str) {
    wirt::plugin::host::show_message(title, message);
}

pub fn log_network_activity(msg: &str) {
    wirt::plugin::host::log_network_activity(msg);
}

/// Delete a content-cache entry (or a trailing-`*` pattern).
///
/// Requires the plugin manifest's `file_write` capability and affects only
/// entries owned by the calling plugin. Every trailing-`*` pattern and every
/// exact raw metadata key also requires `archive_metadata_write`. Denial and
/// cache backend failures return `false`. Product metadata records are
/// unaffected.
pub fn invalidate_cache(key: &str) -> bool {
    wirt::plugin::host::invalidate_cache(key)
}

// === Data API Helpers ===

// Only expose what plugins need - NOT cache internals
pub use wirt::plugin::host::{DataRequest, DataResult, DataStatus, ResourceType};

/// Request data from a URL using the capability-filtered Data API.
///
/// `network` is required for HTTP. Cache/database reads and network-result
/// write-back require their corresponding `file_*` / `archive_metadata_*`
/// capabilities. Content-cache keys resolve only in the calling plugin's
/// private namespace. Guest-returned bodies are capped at 4 MiB.
pub fn request_data(key: &str, url: &str, resource_type: ResourceType) -> String {
    let req = DataRequest {
        key: key.to_string(),
        url: Some(url.to_string()),
        resource_type,
        product_id: None,
        sources: vec![], // Host decides the best sources
    };
    wirt::plugin::host::request_data(&req)
}

/// Set a plugin setting within the host's retained-state quotas.
///
/// The host ignores writes beyond 128 entries, 128-byte keys, 64-KiB values,
/// or 1 MiB aggregate text.
pub fn set_setting(key: &str, value: &str) {
    wirt::plugin::host::set_setting(key, value);
}

/// Poll for the status of a request
pub fn poll_data(request_id: &str) -> DataResult {
    wirt::plugin::host::poll_data(request_id)
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
/// `wirt::plugin::host::has_data(key)` /
/// `wirt::plugin::host::get_data(key)` see the cached entry the
/// same way they would after a `fetch_blocking`, within the calling plugin's
/// private cache namespace.
/// Requires `network` + `file_write`; metadata resources and raw metadata
/// cache keys additionally require `archive_metadata_write`.
pub fn fetch_to_cache(key: &str, url: &str, resource_type: ResourceType) -> Result<(), String> {
    let req = DataRequest {
        key: key.to_string(),
        url: Some(url.to_string()),
        resource_type,
        product_id: None,
        sources: vec![],
    };
    if wirt::plugin::host::fetch_to_cache(&req) {
        Ok(())
    } else {
        Err(format!("fetch_to_cache failed for {}", key))
    }
}

/// Reserved ABI for a future host-UI-authorized cached-blob launch.
///
/// Background plugin calls currently fail closed with
/// `external launch disabled: host UI authorization required`; no blob is
/// written or launched. The manifest must still grant `file_read` before the
/// stable denial is returned. `extension` is retained for ABI compatibility.
pub fn play_cached_blob(key: &str, extension: &str) -> Result<(), String> {
    wirt::plugin::host::play_cached_blob(key, extension)
}

/// Legacy DLSite-only compatibility listing of the first bounded page.
pub fn list_cached_entries() -> Result<Vec<String>, String> {
    Ok(wirt::plugin::host::list_cached_entries())
}

pub fn cached_metadata_count(source: &str) -> Result<u64, String> {
    wirt::plugin::host::cached_metadata_count(source)
}

/// List one source-explicit metadata page. `limit` must be at most 256.
pub fn list_cached_metadata(source: &str, offset: u32, limit: u32) -> Result<Vec<String>, String> {
    wirt::plugin::host::list_cached_metadata(source, offset, limit)
}

/// Re-export MetadataSummary for use in plugins
pub use wirt::plugin::host::MetadataSummary;

/// Batch query for metadata summaries (id, title, geo_blocked).
///
/// Accepts at most 256 ids, each at most 256 bytes, and enforces a 1 MiB
/// aggregate input/output budget before returning guest-owned strings. The
/// database projects only id, bounded title, and geo-blocked fields.
pub fn get_metadata_summaries(ids: Vec<String>) -> Vec<MetadataSummary> {
    wirt::plugin::host::get_metadata_summaries(&ids)
}

pub fn get_metadata_summaries_for_source(
    source: &str,
    ids: Vec<String>,
) -> Result<Vec<MetadataSummary>, String> {
    wirt::plugin::host::get_metadata_summaries_for_source(source, &ids)
}

/// Get full product metadata (maximum 4 MiB).
///
/// `product_id` and `source` are capped at 256 bytes. Resolution order is the
/// local database, calling plugin's JSON/HTML cache, then Gameta. Database
/// reads require `archive_metadata_read`; cached JSON/HTML also requires
/// `file_read`; Gameta additionally requires `network` and consumes one
/// request-budget permit per actual HTTP request. Cache repair/migration
/// persists only with `archive_metadata_write`.
pub fn get_product_metadata(product_id: &str, source: &str) -> Option<String> {
    wirt::plugin::host::get_product_metadata(product_id, source)
}

/// Create a file in private temporary storage owned by this plugin instance.
///
/// Requires `file_write`. `filename` is a sanitized hint; the host selects a
/// collision-safe name. Each instance is limited to 128 files and 64 MiB
/// cumulatively, and its storage is removed on unload. Returns the full path.
pub fn create_file(filename: &str, content: &[u8]) -> Result<String, String> {
    wirt::plugin::host::create_file(filename, content)
}
