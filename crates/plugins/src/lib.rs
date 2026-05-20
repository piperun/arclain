//! Plugin system for Archust
//!
//! This crate provides a WASM-based plugin system that allows extending
//! Archust's functionality through secure, sandboxed plugins.
//!
//! # Architecture
//!
//! - **Plugin Manager**: Discovers, loads, and manages plugin lifecycle
//! - **WASM Runtime**: Executes plugin code safely using wasmtime
//! - **Plugin Loader**: Discovers plugins from the plugins directory
//! - **Event System**: Dispatches archive events to plugins
//! - **Capability System**: Controls what plugins can access
//!
//! # Example
//!
//! ```no_run
//! use arclain_plugins::{PluginManager, PluginEvent};
//! use std::path::PathBuf;
//! use std::collections::HashMap;
//!
//! let plugins_dir = PathBuf::from("plugins");
//! let initial_settings = HashMap::new();
//! let mut manager = PluginManager::new(plugins_dir, initial_settings).unwrap();
//! manager.init().unwrap();
//!
//! // Grab a sender once at startup; events flow through the
//! // background worker without locking the manager.
//! let tx = manager.get_event_sender();
//!
//! tx.send(PluginEvent::OnArchiveOpen {
//!     path: "test.zip".to_string(),
//!     kind: arclain_core::ArchiveKind::Zip,
//!     password: None,
//! }).unwrap();
//! ```

mod conversions;
pub mod host_functions;
pub mod loader;
pub mod manager;
pub mod runtime;
pub mod types;

// Generate bindings from WIT
pub mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit/arclain.wit",
        world: "plugin-world",
    });
}

pub use bindings::arclain;
// `bindings::PluginWorld` is the bindgen-generated trampoline used
// only by `runtime.rs` inside this crate — exposed via `crate::`
// path for runtime.rs's `use crate::PluginWorld`, but kept crate-
// private so it doesn't leak into the public API.
pub(crate) use bindings::PluginWorld;

// Re-export main types
pub use host_functions::HostFunctions;
pub use loader::{DiscoveredPlugin, PluginLoader};
pub use manager::{PluginListItem, PluginManager};
pub use runtime::{LoadedPlugin, PluginInstance, WasmRuntime};
pub use types::{
    BadgeConfig, PluginCapability, PluginError, PluginEvent, PluginInfo, PluginManifest,
    PluginMetadata, PluginResponse, Result,
};
// `types::TopTabConfig` is intentionally not re-exported — all
// internal users access it via `crate::types::TopTabConfig` and no
// external consumer references it.

/// Get the default plugins directory path.
///
/// Crate-private — external callers configure their own paths or go
/// through `PluginManager`. Only consumed by this crate's own
/// `tests` module, so `dead_code` is allowed for non-test builds.
#[allow(dead_code)]
pub(crate) fn default_plugins_dir() -> std::path::PathBuf {
    if let Some(data_dir) = dirs::data_local_dir() {
        data_dir.join("archust").join("plugins")
    } else {
        std::path::PathBuf::from("plugins")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_plugins_dir() {
        let dir = default_plugins_dir();
        assert!(dir.to_string_lossy().contains("plugins"));
    }
}
