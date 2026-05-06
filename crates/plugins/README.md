# Archust Plugin System

A WASM-based plugin system for Archust that enables secure, sandboxed extensibility.

## Overview

The plugin system allows developers to extend Archust's functionality through WebAssembly plugins. Plugins can:

- React to archive lifecycle events (open, close, extract, etc.)
- Process and enrich archive metadata
- Integrate with external services
- Extend functionality without modifying core code

## Architecture

The plugin system consists of several components:

- **Plugin Manager**: Discovers, loads, and manages plugin lifecycle
- **WASM Runtime**: Executes plugin code safely using wasmtime
- **Plugin Loader**: Discovers plugins from the plugins directory
- **Event System**: Dispatches archive events to plugins
- **Capability System**: Controls what plugins can access

## Usage

### Initializing the Plugin System

```rust
use arclain_plugins::{PluginManager, default_plugins_dir};
use std::collections::HashMap;

let plugins_dir = default_plugins_dir();
let mut manager = PluginManager::new(plugins_dir, HashMap::new())?;
manager.init()?;
```

### Dispatching Events

Production code dispatches events through a cloned channel sender;
events are processed by a background worker thread and the call
never blocks. The synchronous `dispatch_event` API on the manager
is retained for tests only.

```rust
use arclain_plugins::PluginEvent;
use arclain_core::ArchiveKind;

let tx = manager.get_event_sender();

tx.send(PluginEvent::OnArchiveOpen {
    path: "test.zip".to_string(),
    kind: ArchiveKind::Zip,
    password: None,
})?;
```

Plugin responses are surfaced through other channels (e.g. the
metadata signal, the network log, or `set_status_message`) rather
than as a synchronous return value.

### Managing Plugins

```rust
// List all plugins
let plugins = manager.list_plugins();
for plugin in plugins {
    println!("Plugin: {} v{}", plugin.name, plugin.version);
}

// Enable/disable plugins
manager.disable_plugin("plugin-id")?;
manager.enable_plugin("plugin-id")?;

// Reload a plugin
manager.reload_plugin("plugin-id")?;
```

## Plugin Development

See the `plugin-sdk` crate for tools to develop plugins.

### Plugin Structure

A plugin consists of two files:

1. **plugin.toml** - Manifest file
2. **plugin.wasm** - Compiled WASM module

### Example Manifest

```toml
[plugin]
id = "my-plugin"
name = "My Plugin"
version = "1.0.0"
author = "Your Name"
description = "A sample plugin"

[capabilities]
network = true
archive_metadata_read = true
archive_metadata_write = true
archive_modify = false
file_read = false
file_write = false

[rate_limits]
http_requests_per_minute = 10
```

## Security

Plugins run in a sandboxed WASM environment with:

- Memory isolation from the host application
- No direct file system access
- Controlled network access through capabilities
- Capability-based permission model

## Events

Plugins can subscribe to these events:

- `OnArchiveOpen` - Archive was opened
- `OnArchiveClose` - Archive was closed
- `OnArchiveList` - Archive contents were listed
- `OnFileExtract` - File was extracted from archive
- `OnFileOpen` - File was opened from archive
- `OnFileAdd` - File was added to archive
- `OnFileDelete` - File was deleted from archive
- `OnMetadataDisplay` - Metadata display requested

## Capabilities

Plugins must declare required capabilities:

- `Network` - Make HTTP requests
- `FileRead` - Read files from archives
- `FileWrite` - Write files to archives
- `ArchiveMetadataRead` - Read archive metadata
- `ArchiveMetadataWrite` - Write archive metadata
- `ArchiveModify` - Modify archive structure

## Implementation Status

### Phase 1: Foundation ✅
- [x] WASM runtime wrapper
- [x] Core type definitions
- [x] Plugin loader
- [x] Plugin manager
- [x] Basic capability system

### Phase 2: Host Functions (In Progress)
- [x] Logging interface
- [x] Network operations (placeholder)
- [ ] File operations (full implementation)
- [ ] Archive metadata operations
- [ ] Archive modification operations

### Phase 3-8: Future Development
- [ ] Plugin SDK
- [ ] Example plugins
- [ ] DLSite metadata plugin
- [ ] Media preview plugin
- [ ] UI integration
- [ ] Documentation

## Testing

Run tests with:

```bash
cargo test -p arclain_plugins
```

## License

Same as Archust main project.