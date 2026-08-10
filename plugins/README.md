# Arclain WASM Plugins

This directory contains WASM plugins that extend Arclain's functionality.

## Plugin Structure

Each plugin is organized in its own folder:

```
plugins/
├── dlsite-metadata/           # DLSite metadata enrichment plugin
│   ├── Cargo.toml            # Rust project configuration
│   ├── dlsite-metadata.toml  # Plugin manifest
│   ├── dlsite-metadata.wasm  # Compiled WASM binary
│   └── src/
│       └── lib.rs            # Plugin source code
└── gstreamer-preview/         # Media preview plugin
    ├── Cargo.toml
    ├── gstreamer-preview.toml
    ├── gstreamer-preview.wasm
    └── src/
        └── lib.rs
```

## Building Plugins

### Prerequisites

1. Install the WASM target:
   ```bash
   rustup target add wasm32-wasip2
   ```

### Build All Plugins

```bash
python scripts/release.py plugins
```

This installs the target if missing, builds every plugin, and copies each `<name>.wasm` next to its `Cargo.toml`.

To wipe `.wasm` artifacts and `cargo clean` every plugin:

```bash
python scripts/release.py clean-plugins
```

### Build Individual Plugin

```bash
cd plugins/dlsite-metadata
cargo build --target wasm32-wasip2 --release
```

The compiled `.wasm` file will be in `target/wasm32-wasip2/release/`

## Plugin Manifest Format

Each plugin requires a `.toml` manifest file with the same name as the plugin folder:

```toml
[wirt]
abi = "0.1.0"

[plugin]
id = "plugin-id"
name = "Plugin Name"
version = "1.0.0"
author = "Author Name"
description = "Plugin description"

[capabilities]
network = true                  # HTTP requests
network_domains = ["example.invalid"] # Exact allowed domains
archive_metadata_read = true    # Read archive metadata
archive_metadata_write = true   # Write archive metadata
archive_modify = false          # Modify archive contents
file_read = false               # Read cached blobs / approved local files
file_write = false              # Create private temp files / delete cache entries

[rate_limits]
http_requests_per_minute = 10   # HTTP rate limit
```

## Available Plugins

### DLSite Metadata

Extracts DLSite product codes (RJ/VJ/BJ) from archive filenames and enriches them with metadata from the DLSite API.

**Features:**
- Automatic code detection from filenames
- Metadata fetching from DLSite
- Title extraction from filenames as fallback

**Supported codes:**
- RJ123456 (Doujin works)
- VJ01234567 (Voice works)
- BJ12345678 (Books/Comics)

### GStreamer Preview

Provides media file previews and thumbnail generation for video and audio files.

**Features:**
- Video thumbnail generation
- Audio metadata extraction
- Media file detection

**Note:** Currently a coordinator plugin that delegates to a native GStreamer service.

## Creating a New Plugin

Wirt is the product-neutral plugin API, SDK, and ABI. It defines the host /
plugin boundary; Wirt is not a plugin itself. Depend on `wirt-sdk` to generate
guest bindings for that ABI.

1. **Create Plugin Directory**
   ```bash
   mkdir plugins/my-plugin
   cd plugins/my-plugin
   ```

2. **Initialize Cargo Project**
   ```bash
   cargo init --lib
   ```

3. **Update Cargo.toml**
   ```toml
   [package]
   name = "my-plugin"
   version = "0.1.0"
   edition = "2021"

   [lib]
   crate-type = ["cdylib"]

   [dependencies]
   wirt-sdk = { path = "../../wirt-sdk" }
   serde = { version = "1", features = ["derive", "alloc"], default-features = false }
   serde_json = { version = "1", features = ["alloc"], default-features = false }

   [profile.release]
   opt-level = "z"
   lto = true
   strip = true
   panic = "abort"
   codegen-units = 1
   ```

4. **Create Plugin Manifest** (`my-plugin.toml`)
   ```toml
   [wirt]
   abi = "0.1.0"

   [plugin]
   id = "my-plugin"
   name = "My Plugin"
   version = "0.1.0"
   author = "Your Name"
   description = "Plugin description"

   [capabilities]
   network = false
   archive_metadata_read = true
   archive_metadata_write = false
   archive_modify = false
   file_read = false
   file_write = false

   [rate_limits]
   http_requests_per_minute = 10
   ```

5. **Write Plugin Code** (`src/lib.rs`)
   ```rust
   #![no_std]

   extern crate alloc;
   use alloc::string::String;
   use wirt_sdk::*;
   use serde_json::json;

   plugin_metadata!(
       "my-plugin",
       "My Plugin",
       "0.1.0",
       "Your Name",
       "Plugin description"
   );

   plugin_init!();
   plugin_cleanup!();

   #[no_mangle]
   pub extern "C" fn plugin_on_event(event_ptr: *const u8, event_len: usize) -> i32 {
       // Parse event
       let event_bytes = unsafe {
           core::slice::from_raw_parts(event_ptr, event_len)
       };
       
       let event_str = match core::str::from_utf8(event_bytes) {
           Ok(s) => s,
           Err(_) => return -1,
       };
       
       let event: PluginEvent = match serde_json::from_str(event_str) {
           Ok(e) => e,
           Err(_) => return -1,
       };
       
       // Handle events
       match event {
           PluginEvent::OnArchiveOpen { path, .. } => {
               log(LogLevel::Info, &format!("Archive opened: {}", path));
               0
           }
           _ => 0
       }
   }
   ```

6. **Build Plugin**
   ```bash
   cargo build --target wasm32-unknown-unknown --release
   cp target/wasm32-unknown-unknown/release/my_plugin.wasm my-plugin.wasm
   ```

7. **Test Plugin**
   - The `.wasm` file will be automatically discovered by Archust
   - Check the Plugins settings page to verify it loaded
   - Enable the plugin and test with archives

## Wirt SDK API

### Host Functions

**Logging:**
```rust
log(LogLevel::Info, "Message");
log(LogLevel::Error, "Error message");
```

All plugin log levels are subject to host entry, rate, and daily-byte caps.
Admitted warning/error entries also reach application tracing; lower levels
remain in the per-plugin file log.

**HTTP Requests:**
```rust
// GET request
match http_get("https://api.example.com/data") {
    Ok(response) => { /* process response */ }
    Err(code) => { /* handle error */ }
}

// POST request
let body = json!({"key": "value"}).to_string();
match http_post("https://api.example.com/endpoint", &body) {
    Ok(response) => { /* process response */ }
    Err(code) => { /* handle error */ }
}
```

**File Operations (`file_write` capability):**
```rust
// Creates a collision-safe file in per-plugin temporary storage.
// The filename is a hint, not an arbitrary host path.
let path = create_file("export.json", br#"{"ok":true}"#)?;

// Deletes only this plugin's exact ordinary content-cache key.
let removed = invalidate_cache("my-plugin:stale-entry");

// Every trailing-* pattern and every exact raw metadata key additionally
// requires archive_metadata_write.
let removed_group = invalidate_cache("my-plugin:stale-prefix:*");
```

**Data API capabilities:** MetadataStore reads require
`archive_metadata_read`; ContentCache and LocalFile reads require `file_read`;
Network requires `network`. MetadataStore write-back requires
`archive_metadata_write`, while ContentCache write-back requires `file_write`.
Raw `:json:`/`:html:` metadata cache access also requires the corresponding
archive metadata capability. `:metadata:` is treated the same way; markers are
matched structurally and ASCII-case-insensitively without percent-decoding.
Every content-cache read, write, and invalidation is confined to the calling
plugin's namespace. Guest-returned bodies are limited to 4 MiB.

Metadata APIs are bounded: cached-entry listing returns at most 1024 ids and
1 MiB from a limited query; summary queries accept at most 256 ids of 256 bytes
each, project only id/bounded title/status, and enforce a 1 MiB aggregate
budget; product ids and sources are capped at 256 bytes. Product lookup order
is local database, the calling plugin's JSON/HTML cache, then Gameta, with one
network permit per actual HTTP request.

The legacy `show_message` import writes only to the bounded plugin log, and
`set_status_message` is a deprecated no-op. Plugins should return UI actions
or use bounded logging instead.

### Plugin Events

**OnArchiveOpen:**
```rust
PluginEvent::OnArchiveOpen { path, kind } => {
    // Called when an archive is opened
    // path: String - Full path to archive
    // kind: ArchiveKind - Type of archive (Zip, Rar, etc.)
}
```

**OnFileExtract:**
```rust
PluginEvent::OnFileExtract { path, dest } => {
    // Called when a file is extracted
    // path: String - Path within archive
    // dest: String - Extraction destination
}
```

**OnMetadataDisplay:**
```rust
PluginEvent::OnMetadataDisplay { path } => {
    // Called when metadata should be displayed
    // Return PluginResponse::Metadata with data
}
```

### Plugin Responses

**Metadata Response:**
```rust
let metadata = json!({
    "key1": "value1",
    "key2": "value2"
});

PluginResponse::Metadata { data: metadata }
```

**Success Response:**
```rust
PluginResponse::Success
```

**Error Response:**
```rust
PluginResponse::Error { 
    message: "Error description".to_string() 
}
```

## Debugging Plugins

1. **Check Logs**
   - Plugins log to the Archust log file
   - Use `log(LogLevel::Debug, "message")` for debugging

2. **Verify WASM File**
   ```bash
   ls -lh plugins/*/*.wasm
   ```

3. **Test Loading**
   - Open Archust
   - Go to Settings → Plugins
   - Verify plugin appears in the list
   - Check enable/disable functionality

4. **Test Events**
   - Open an archive that matches your plugin's criteria
   - Check if events are being handled (via logs)

## Performance Tips

1. **Optimize Binary Size**
   - Already configured in `Cargo.toml` profile
   - Use `cargo bloat` to analyze binary size

2. **Minimize Host Function Calls**
   - Cache data when possible
   - Batch operations

3. **Efficient String Handling**
   - Use `&str` instead of `String` when possible
   - Avoid unnecessary allocations

## Security Considerations

1. **Capability System**
   - Only request capabilities you need
   - Plugins can't access anything outside their capabilities

2. **Rate Limiting**
   - HTTP requests are rate-limited per plugin
   - Respect the configured limits

3. **Sandboxing**
   - Plugins run in WASM sandbox
   - No direct file system or network access
   - All operations go through host functions

## Troubleshooting

**Plugin doesn't appear in list:**
- Verify `.toml` and `.wasm` files have matching names
- Check that files are in correct directory structure
- Review plugin manifest for errors

**Plugin fails to load:**
- Check Archust logs for error messages
- Verify WASM file is valid (not corrupted)
- Ensure all dependencies are `no_std` compatible

**Events not firing:**
- Verify plugin is enabled
- Check that event types are handled
- Review capability permissions

**HTTP requests failing:**
- Check network capability is enabled
- Verify rate limits
- Check URL format and accessibility

## Examples

See the existing plugins for complete examples:
- **dlsite-metadata**: Complex plugin with HTTP requests and regex parsing
- **gstreamer-preview**: Coordinator plugin with native service integration

## Resources

- [Wirt SDK source](../wirt-sdk/src/lib.rs)
- [Wasmtime Documentation](https://docs.wasmtime.dev/)
- [Rust WASM Book](https://rustwasm.github.io/docs/book/)

## Support

For questions or issues with plugin development, please open an issue on the Archust GitHub repository.
