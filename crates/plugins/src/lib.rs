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
//! 
//! let plugins_dir = PathBuf::from("plugins");
//! let mut manager = PluginManager::new(plugins_dir).unwrap();
//! manager.init().unwrap();
//! 
//! // Dispatch an event to all plugins
//! let event = PluginEvent::OnArchiveOpen {
//!     path: "test.zip".to_string(),
//!     kind: arclain_core::ArchiveKind::Zip,
//! };
//! 
//! let responses = manager.dispatch_event(&event);
//! ```

pub mod loader;
pub mod manager;
pub mod runtime;
pub mod types;
pub mod host_functions;

// Re-export main types
pub use loader::{DiscoveredPlugin, PluginLoader};
pub use manager::{PluginListItem, PluginManager};
pub use runtime::{LoadedPlugin, PluginInstance, WasmRuntime};
pub use types::{
    PluginCapability, PluginError, PluginEvent, PluginInfo, PluginManifest, PluginMetadata,
    PluginResponse, Result,
};
pub use host_functions::HostFunctions;

/// Get the default plugins directory path
pub fn default_plugins_dir() -> std::path::PathBuf {
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