//! WASM runtime wrapper using wasmtime
//!
//! This module provides the WASM runtime with full host function support.

use crate::host_functions::{self, HostFunctions};
use crate::types::{
    PluginCapability, PluginError, PluginEvent, PluginExtensionPoint, PluginMetadata,
    PluginResponse, PluginUiElement, Result,
};
use arclain_core::sevenzip::SevenZipCli;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info};
use wasmtime::*;

/// WASM runtime for executing plugins
pub struct WasmRuntime {
    engine: Engine,
}

impl WasmRuntime {
    /// Create a new WASM runtime
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_backtrace_details(WasmBacktraceDetails::Enable);

        let engine = Engine::new(&config).map_err(|e| PluginError::WasmError(e.to_string()))?;

        info!("WASM runtime initialized");
        Ok(Self { engine })
    }

    /// Load a WASM module from a file
    pub fn load_module(&self, path: &Path) -> Result<LoadedPlugin> {
        debug!("Loading WASM module from: {}", path.display());

        let module_bytes = std::fs::read(path)
            .map_err(|e| PluginError::LoadError(format!("Failed to read WASM file: {}", e)))?;

        let module = Module::new(&self.engine, &module_bytes)
            .map_err(|e| PluginError::LoadError(e.to_string()))?;

        info!("WASM module loaded successfully: {}", path.display());

        Ok(LoadedPlugin {
            module,
            engine: self.engine.clone(),
            _path: path.to_path_buf(),
        })
    }

    /// Load a WASM module from bytes
    pub fn load_module_from_bytes(&self, bytes: &[u8]) -> Result<LoadedPlugin> {
        debug!("Loading WASM module from bytes ({} bytes)", bytes.len());

        let module =
            Module::new(&self.engine, bytes).map_err(|e| PluginError::LoadError(e.to_string()))?;

        info!("WASM module loaded successfully from bytes");

        Ok(LoadedPlugin {
            module,
            engine: self.engine.clone(),
            _path: std::path::PathBuf::from("<bytes>"),
        })
    }
}

/// A loaded WASM plugin ready for execution
pub struct LoadedPlugin {
    module: Module,
    engine: Engine,
    _path: std::path::PathBuf,
}

impl LoadedPlugin {
    /// Create a new plugin instance
    ///
    /// Full host function support with HTTP, file operations, and logging
    pub fn instantiate(
        &self,
        capabilities: Vec<PluginCapability>,
        requests_per_minute: u32,
    ) -> Result<PluginInstance> {
        self.instantiate_with_backend(capabilities, requests_per_minute, None)
    }

    /// Create a new plugin instance with archive backend
    ///
    /// Full host function support including archive file operations
    pub fn instantiate_with_backend(
        &self,
        capabilities: Vec<PluginCapability>,
        requests_per_minute: u32,
        backend: Option<Arc<SevenZipCli>>,
    ) -> Result<PluginInstance> {
        // Create host functions state
        let host_funcs = if let Some(backend) = backend {
            HostFunctions::with_backend(
                capabilities.into_iter().collect(),
                requests_per_minute,
                backend,
            )
        } else {
            HostFunctions::new(capabilities.into_iter().collect(), requests_per_minute)
        };

        let mut store = Store::new(&self.engine, host_funcs);
        let mut linker = Linker::new(&self.engine);

        // Register host functions
        linker
            .func_wrap("env", "host_log", host_functions::host_log)
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        linker
            .func_wrap("env", "host_http_get", host_functions::host_http_get)
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        linker
            .func_wrap(
                "env",
                "host_http_post_json",
                host_functions::host_http_post_json,
            )
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        linker
            .func_wrap("env", "host_file_read", host_functions::host_file_read)
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        linker
            .func_wrap("env", "host_file_write", host_functions::host_file_write)
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        linker
            .func_wrap(
                "env",
                "host_archive_metadata_get",
                host_functions::host_archive_metadata_get,
            )
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        linker
            .func_wrap(
                "env",
                "host_archive_metadata_set",
                host_functions::host_archive_metadata_set,
            )
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        linker
            .func_wrap("env", "__rust_dealloc", host_functions::host_dealloc)
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        linker
            .func_wrap("env", "__rust_alloc", host_functions::host_alloc)
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        linker
            .func_wrap("env", "__wasm_dealloc", host_functions::host_wasm_dealloc)
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        linker
            .func_wrap("env", "__wasm_alloc", host_functions::host_wasm_alloc)
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        // Instantiate the module
        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        debug!("Plugin instance created with host functions");

        Ok(PluginInstance {
            store,
            instance,
            metadata: None,
        })
    }
}

/// An instantiated plugin that can receive events
pub struct PluginInstance {
    store: Store<HostFunctions>,
    instance: Instance,
    metadata: Option<PluginMetadata>,
}

impl PluginInstance {
    /// Initialize the plugin
    pub fn init(&mut self) -> Result<()> {
        let init_func = self
            .instance
            .get_typed_func::<(), i32>(&mut self.store, "plugin_init")
            .map_err(|e| PluginError::InitError(format!("Missing plugin_init function: {}", e)))?;

        let result = init_func
            .call(&mut self.store, ())
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        if result != 0 {
            return Err(PluginError::InitError(format!(
                "plugin_init returned error code: {}",
                result
            )));
        }

        debug!("Plugin initialized successfully");
        Ok(())
    }

    /// Get plugin metadata
    pub fn get_metadata(&mut self) -> Result<PluginMetadata> {
        // Check if metadata is already cached
        if let Some(metadata) = &self.metadata {
            return Ok(metadata.clone());
        }

        // Call plugin_metadata function to get metadata
        let metadata_func = self
            .instance
            .get_typed_func::<(u32, u32), i32>(&mut self.store, "plugin_metadata")
            .map_err(|e| {
                PluginError::InitError(format!("Missing plugin_metadata function: {}", e))
            })?;

        // Allocate buffer for metadata JSON
        const BUFFER_SIZE: u32 = 4096;
        let memory = self
            .instance
            .get_memory(&mut self.store, "memory")
            .ok_or_else(|| PluginError::ExecutionError("No memory export found".to_string()))?;

        // Allocate a buffer in WASM memory (simplified - assumes memory is already allocated)
        // In production, we'd call a WASM allocator function
        let buffer_ptr = 1024u32; // Fixed offset for now

        let result = metadata_func
            .call(&mut self.store, (buffer_ptr, BUFFER_SIZE))
            .map_err(|e| PluginError::ExecutionError(e.to_string()))?;

        if result < 0 {
            return Err(PluginError::ExecutionError(
                "Plugin failed to provide metadata".to_string(),
            ));
        }

        // Read metadata JSON from WASM memory
        let mut buffer = vec![0u8; result as usize];
        memory
            .read(&self.store, buffer_ptr as usize, &mut buffer)
            .map_err(|e| PluginError::ExecutionError(format!("Failed to read metadata: {}", e)))?;

        let metadata: PluginMetadata = serde_json::from_slice(&buffer)
            .map_err(|e| PluginError::ExecutionError(format!("Invalid metadata JSON: {}", e)))?;

        self.metadata = Some(metadata.clone());

        Ok(metadata)
    }

    /// Dispatch an event to the plugin
    pub fn on_event(&mut self, _event: &PluginEvent) -> Result<PluginResponse> {
        // Phase 1: Return placeholder response
        // Full event dispatch will be implemented in Phase 2
        Ok(PluginResponse::None)
    }

    /// Get UI layout for a specific extension point
    ///
    /// Returns a vector of `PluginUiElement`
    pub fn get_ui_layout(
        &mut self,
        extension_point: PluginExtensionPoint,
    ) -> Result<Vec<PluginUiElement>> {
        // Check if the plugin exports the ui_layout function
        let ui_layout_func = match self
            .instance
            .get_typed_func::<(u32, u32, u32), i32>(&mut self.store, "plugin_ui_layout")
        {
            Ok(f) => f,
            Err(_) => {
                // Plugin doesn't provide UI, return empty layout
                return Ok(Vec::new());
            }
        };

        // Allocate buffer for UI JSON
        const BUFFER_SIZE: u32 = 8192;
        let memory = self
            .instance
            .get_memory(&mut self.store, "memory")
            .ok_or_else(|| PluginError::ExecutionError("No memory export found".to_string()))?;

        let buffer_ptr = 1024u32;

        // Convert extension point to u32 for WASM
        let extension_point_id = match extension_point {
            PluginExtensionPoint::MainPage => 0,
            PluginExtensionPoint::Sidebar => 1,
            PluginExtensionPoint::ContextMenu => 2,
        };

        let result = ui_layout_func
            .call(
                &mut self.store,
                (extension_point_id, buffer_ptr, BUFFER_SIZE),
            )
            .map_err(|e| PluginError::ExecutionError(e.to_string()))?;

        if result < 0 {
            return Err(PluginError::ExecutionError(
                "Plugin failed to provide UI layout".to_string(),
            ));
        }

        if result == 0 {
            // Plugin doesn't provide UI for this extension point
            return Ok(Vec::new());
        }

        // Read UI layout JSON from WASM memory
        let mut buffer = vec![0u8; result as usize];
        memory
            .read(&self.store, buffer_ptr as usize, &mut buffer)
            .map_err(|e| PluginError::ExecutionError(format!("Failed to read UI layout: {}", e)))?;

        let ui_elements: Vec<PluginUiElement> = serde_json::from_slice(&buffer)
            .map_err(|e| PluginError::ExecutionError(format!("Invalid UI layout JSON: {}", e)))?;

        Ok(ui_elements)
    }

    /// Send a UI event to the plugin (e.g., button click, text input change)
    ///
    /// # Arguments
    /// * `element_id` - The ID of the UI element that triggered the event
    /// * `value` - Optional value (e.g., new text for TextInput, checked state for Checkbox)
    ///
    /// # Returns
    /// Returns `true` if the plugin successfully handled the event
    pub fn send_ui_event(&mut self, element_id: &str, value: Option<String>) -> Result<bool> {
        // Check if the plugin exports the ui_event function
        let ui_event_func = match self
            .instance
            .get_typed_func::<(u32, u32, u32, u32), i32>(&mut self.store, "plugin_on_ui_event")
        {
            Ok(f) => f,
            Err(_) => {
                // Plugin doesn't handle UI events
                return Ok(false);
            }
        };

        let memory = self
            .instance
            .get_memory(&mut self.store, "memory")
            .ok_or_else(|| PluginError::ExecutionError("No memory export found".to_string()))?;

        // Write element_id to memory
        let id_bytes = element_id.as_bytes();
        let id_ptr = 1024u32;
        memory
            .write(&mut self.store, id_ptr as usize, id_bytes)
            .map_err(|e| {
                PluginError::ExecutionError(format!("Failed to write element ID: {}", e))
            })?;

        // Write value to memory (if provided)
        let (value_ptr, value_len) = if let Some(val) = value {
            let value_bytes = val.as_bytes();
            let vptr = 2048u32;
            memory
                .write(&mut self.store, vptr as usize, value_bytes)
                .map_err(|e| {
                    PluginError::ExecutionError(format!("Failed to write value: {}", e))
                })?;
            (vptr, value_bytes.len() as u32)
        } else {
            (0u32, 0u32)
        };

        let result = ui_event_func
            .call(
                &mut self.store,
                (id_ptr, id_bytes.len() as u32, value_ptr, value_len),
            )
            .map_err(|e| PluginError::ExecutionError(e.to_string()))?;

        Ok(result == 0)
    }

    /// Clean up the plugin
    pub fn cleanup(&mut self) -> Result<()> {
        if let Ok(cleanup_func) = self
            .instance
            .get_typed_func::<(), ()>(&mut self.store, "plugin_cleanup")
        {
            cleanup_func
                .call(&mut self.store, ())
                .map_err(|e| PluginError::ExecutionError(e.to_string()))?;

            debug!("Plugin cleanup completed");
        }

        Ok(())
    }
}

impl Drop for PluginInstance {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_creation() {
        let runtime = WasmRuntime::new();
        assert!(runtime.is_ok());
    }
}
