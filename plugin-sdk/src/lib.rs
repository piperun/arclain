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
