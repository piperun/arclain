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

// HTTP helpers
pub fn http_get(url: &str) -> Result<String, String> {
    arclain::plugin::host::http_get(url)
}

pub fn http_post(url: &str, body: &str) -> Result<String, String> {
    arclain::plugin::host::http_post(url, body)
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

// Caching helpers
pub fn get_cached_metadata(id: &str) -> Option<String> {
    arclain::plugin::host::get_cached_metadata(id)
}

pub fn save_cached_metadata(id: &str, json: &str) {
    arclain::plugin::host::save_cached_metadata(id, json);
}

pub fn list_cached_entries() -> Vec<String> {
    arclain::plugin::host::list_cached_entries()
}

pub fn export_cache() -> Result<String, String> {
    arclain::plugin::host::export_cache()
}

pub fn import_cache() -> Result<String, String> {
    arclain::plugin::host::import_cache()
}

pub fn start_async_fetch(url: &str) -> String {
    arclain::plugin::host::start_async_fetch(url)
}

pub fn poll_async_fetch(id: &str) -> Option<Result<String, String>> {
    arclain::plugin::host::poll_async_fetch(id)
}
