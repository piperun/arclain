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
