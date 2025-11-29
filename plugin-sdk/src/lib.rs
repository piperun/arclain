//! Archust Plugin SDK
//!
//! This SDK provides types and host function wrappers for building Archust plugins.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

// Re-export commonly used types
pub use serde_json;

/// Prelude module for easy import
pub mod prelude {
    pub use super::{
        http_get, http_post_json, log, LogLevel, PluginEvent, PluginExtensionPoint, PluginResponse,
        PluginUiElement,
    };
}

// Global allocator for WASM
use core::alloc::{GlobalAlloc, Layout};

struct WasmAllocator;

unsafe impl GlobalAlloc for WasmAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Use WASM's built-in memory allocation
        // This is a simple bump allocator that works with WASM linear memory
        extern "C" {
            fn __rust_alloc(size: usize, align: usize) -> *mut u8;
        }
        __rust_alloc(layout.size(), layout.align())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        extern "C" {
            fn __rust_dealloc(ptr: *mut u8, size: usize, align: usize);
        }
        __rust_dealloc(ptr, layout.size(), layout.align())
    }
}

#[global_allocator]
static ALLOCATOR: WasmAllocator = WasmAllocator;

// Panic handler for no_std
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // In WASM, we can't really do much on panic
    // Just loop forever (WASM will trap)
    loop {}
}

/// Log levels for host logging
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
    Trace = 4,
}

/// Plugin events from the host
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginEvent {
    OnArchiveOpen { path: String },
    OnArchiveClose { path: String },
    OnFileExtract { archive: String, file_path: String },
    OnInit,
    OnShutdown,
}

/// Plugin response types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginResponse {
    None,
    Metadata { data: serde_json::Value },
    Error { message: String },
}

// External host functions
extern "C" {
    fn host_log(level: u32, ptr: *const u8, len: usize) -> i32;
    fn host_http_get(
        url_ptr: *const u8,
        url_len: usize,
        out_ptr: *mut u8,
        out_max_len: usize,
    ) -> i32;
    fn host_http_post_json(
        url_ptr: *const u8,
        url_len: usize,
        body_ptr: *const u8,
        body_len: usize,
        out_ptr: *mut u8,
        out_max_len: usize,
    ) -> i32;
}

/// Log a message to the host
pub fn log(level: LogLevel, message: &str) {
    unsafe {
        host_log(level as u32, message.as_ptr(), message.len());
    }
}

/// Make an HTTP GET request
pub fn http_get(url: &str) -> Result<String, i32> {
    const BUFFER_SIZE: usize = 65536; // 64KB buffer
    let mut buffer = Vec::with_capacity(BUFFER_SIZE);
    buffer.resize(BUFFER_SIZE, 0u8);

    let result =
        unsafe { host_http_get(url.as_ptr(), url.len(), buffer.as_mut_ptr(), BUFFER_SIZE) };

    if result < 0 {
        Err(result)
    } else {
        let len = result as usize;
        buffer.truncate(len);
        String::from_utf8(buffer).map_err(|_| -100)
    }
}

/// Make an HTTP POST request with JSON body
pub fn http_post_json(url: &str, body: &str) -> Result<String, i32> {
    const BUFFER_SIZE: usize = 65536; // 64KB buffer
    let mut buffer = Vec::with_capacity(BUFFER_SIZE);
    buffer.resize(BUFFER_SIZE, 0u8);

    let result = unsafe {
        host_http_post_json(
            url.as_ptr(),
            url.len(),
            body.as_ptr(),
            body.len(),
            buffer.as_mut_ptr(),
            BUFFER_SIZE,
        )
    };

    if result < 0 {
        Err(result)
    } else {
        let len = result as usize;
        buffer.truncate(len);
        String::from_utf8(buffer).map_err(|_| -100)
    }
}

/// Macro to define plugin metadata
#[macro_export]
macro_rules! plugin_metadata {
    ($id:expr, $name:expr, $version:expr, $author:expr, $description:expr) => {
        #[no_mangle]
        pub extern "C" fn plugin_metadata(out_ptr: *mut u8, out_max_len: u32) -> i32 {
            use alloc::format;

            let metadata = format!(
                r#"{{"id":"{}","name":"{}","version":"{}","author":"{}","description":"{}"}}"#,
                $id, $name, $version, $author, $description
            );

            let bytes = metadata.as_bytes();
            if bytes.len() > out_max_len as usize {
                return -1;
            }

            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), out_ptr, bytes.len());
            }

            bytes.len() as i32
        }
    };
}

/// Macro to define plugin initialization
#[macro_export]
macro_rules! plugin_init {
    ($handler:expr) => {
        #[no_mangle]
        pub extern "C" fn plugin_init() -> i32 {
            match $handler() {
                Ok(()) => 0,
                Err(_) => -1,
            }
        }
    };
    () => {
        #[no_mangle]
        pub extern "C" fn plugin_init() -> i32 {
            0
        }
    };
}

/// Macro to define plugin cleanup
#[macro_export]
macro_rules! plugin_cleanup {
    () => {
        #[no_mangle]
        pub extern "C" fn plugin_cleanup() {
            // Cleanup code here
        }
    };
}

/// Extension point where a plugin provides UI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginExtensionPoint {
    /// Main page when plugin is selected in Plugins page
    MainPage,
    /// Widget to inject into archive properties sidebar
    Sidebar,
    /// Future: context menu items
    ContextMenu,
}

/// UI element that a plugin can define
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginUiElement {
    /// Vertical layout container
    Column {
        #[serde(default)]
        children: Vec<PluginUiElement>,
    },
    /// Horizontal layout container
    Row {
        #[serde(default)]
        children: Vec<PluginUiElement>,
    },
    /// Text label
    Label {
        text: String,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        size: Option<f32>,
    },
    /// Button
    Button { id: String, label: String },
    /// Text input
    TextInput {
        id: String,
        label: String,
        value: String,
    },
    /// Checkbox
    Checkbox {
        id: String,
        label: String,
        checked: bool,
    },
    /// Separator line
    Separator,
    /// Spacing
    Space {
        #[serde(default = "default_space_size")]
        size: f32,
    },
}

fn default_space_size() -> f32 {
    8.0
}

/// Macro to define plugin UI layout
#[macro_export]
macro_rules! plugin_ui_layout {
    ($handler:expr) => {
        #[no_mangle]
        pub extern "C" fn plugin_ui_layout(
            extension_point_id: u32,
            out_ptr: *mut u8,
            out_max_len: u32,
        ) -> i32 {
            use alloc::vec::Vec;
            use $crate::{PluginExtensionPoint, PluginUiElement};

            let extension_point = match extension_point_id {
                0 => PluginExtensionPoint::MainPage,
                1 => PluginExtensionPoint::Sidebar,
                2 => PluginExtensionPoint::ContextMenu,
                _ => return 0,
            };

            let elements: Vec<PluginUiElement> = $handler(extension_point);
            if elements.is_empty() {
                return 0;
            }

            let json = match $crate::serde_json::to_string(&elements) {
                Ok(j) => j,
                Err(_) => return -1,
            };

            let bytes = json.as_bytes();
            if bytes.len() > out_max_len as usize {
                return -1;
            }

            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), out_ptr, bytes.len());
            }

            bytes.len() as i32
        }
    };
}

/// Macro to define plugin UI event handler
#[macro_export]
macro_rules! plugin_ui_event {
    ($handler:expr) => {
        #[no_mangle]
        pub extern "C" fn plugin_on_ui_event(
            id_ptr: *const u8,
            id_len: u32,
            value_ptr: *const u8,
            value_len: u32,
        ) -> i32 {
            use alloc::slice;
            use alloc::string::String;

            let id = unsafe {
                let bytes = slice::from_raw_parts(id_ptr, id_len as usize);
                String::from_utf8_lossy(bytes).into_owned()
            };

            let value = if value_len > 0 {
                unsafe {
                    let bytes = slice::from_raw_parts(value_ptr, value_len as usize);
                    Some(String::from_utf8_lossy(bytes).into_owned())
                }
            } else {
                None
            };

            $handler(&id, value);
            0
        }
    };
}
