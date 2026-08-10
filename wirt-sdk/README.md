# Wirt SDK

`wirt-sdk` provides generated Rust guest bindings and small convenience
helpers for the Wirt plugin API/SDK/ABI. Wirt is product-neutral: it describes
the boundary a host implements and a plugin consumes. It is not a plugin.

The sole ABI source is [`wit/plugin.wit`](wit/plugin.wit). Host bindings,
guest bindings, package preflight, and the checked schema projection all derive
from that file. Do not add another WIT copy.

## Start a project

From the repository root:

```console
rustup target add wasm32-wasip2
just wirt new plugins/my-plugin
```

The new project is standalone and contains a vendored copy of this SDK. The
maintained starter demonstrates the required `Guest` implementation and
`wirt_sdk::export!` call.

For a repository-maintained project, depend on the shared SDK instead:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
wirt-sdk = { path = "../../wirt-sdk" }
```

Build for `wasm32-wasip2`; do not target `wasm32-unknown-unknown`.

## Binding surface

Generated bindings are re-exported at the crate root. Common paths include:

- `wirt_sdk::Guest` for the component implementation.
- `wirt_sdk::export!` for the component export macro.
- `wirt_sdk::wirt::plugin::meta` for metadata types.
- `wirt_sdk::wirt::plugin::rules` for organization rules.
- `wirt_sdk::wirt::plugin::ui` for layouts, elements, actions, and top tabs.
- `wirt_sdk::wirt::plugin::host` for the complete low-level host interface.

Prefer the SDK helpers below where one exists. They document the capability
and quota behavior expected by the current host.

## Logging and settings

```rust
wirt_sdk::debug("diagnostic details");
wirt_sdk::info("work started");
wirt_sdk::warn("using a fallback");
wirt_sdk::error("operation failed");

wirt_sdk::set_setting("mode", "compact");
```

Plugin logs are bounded. Warning and error entries admitted by the host also
reach application tracing; lower levels stay in the per-plugin log. Settings
are limited to 128 entries, 128-byte keys, 64-KiB values, and 1 MiB total
retained text.

## Archive context and metadata

These helpers require `archive_metadata_read` unless stated otherwise:

```rust
let archive = wirt_sdk::current_archive_info();
let count = wirt_sdk::archive_file_count()?;
let first_page = wirt_sdk::list_archive_files_page(0, 256)?;
```

Archive pages contain at most 256 paths and 1 MiB of path text. Use count and
paging rather than assuming one unbounded listing.

Publishing metadata requires `archive_metadata_write`:

```rust
let accepted = wirt_sdk::emit_metadata_for_source("dlsite", metadata_json);
```

Prefer the source-explicit helper. The host checks the active archive source,
input size, 120-writes-per-minute limit, 1,024 distinct IDs per session, and
64-MiB session byte budget.

`rename_archive` requires `archive_modify` and accepts a filename, not an
arbitrary destination path.

## Data and network

The data API exposes capability-filtered resources:

```rust
use wirt_sdk::{fetch_blocking, fetch_to_cache, ResourceType};

let json = fetch_blocking(
    "my-plugin:item:42",
    "https://api.example.invalid/items/42",
    ResourceType::Json,
)?;

fetch_to_cache(
    "my-plugin:blob:42",
    "https://cdn.example.invalid/blob/42",
    ResourceType::ContentCache,
)?;
```

- HTTP requires `network`, an exact approved manifest domain, and a request
  permit under `http_requests_per_minute`.
- Content-cache and approved local-file reads require `file_read`.
- Content-cache writes require `file_write`.
- Structured or raw metadata reads/writes additionally require the matching
  archive metadata capability.
- Cache keys are confined to the calling plugin's namespace.
- Bodies returned to the guest are limited to 4 MiB.

Use `fetch_to_cache` for large blobs so bytes do not cross the component ABI.
`play_cached_blob` is reserved for a future host-UI-authorized flow and
currently fails closed.

Metadata collection helpers are also bounded: pages are at most 256 entries,
cached-entry lists at most 1,024 IDs, summary queries at most 256 IDs of 256
bytes each, and aggregate collection text at most 1 MiB.

## Private file creation and cache invalidation

`create_file` requires `file_write` and creates a collision-safe file in the
plugin's private temporary storage:

```rust
let path = wirt_sdk::create_file("export.json", br#"{"ok":true}"#)?;
```

The filename is a hint, not a host path. One instance may retain at most 128
temporary files totaling 64 MiB; safe filename hints are limited to 96 bytes.

`invalidate_cache` affects only the caller's namespace. It requires
`file_write`; metadata keys and trailing-`*` patterns also require
`archive_metadata_write`.

## Result and runtime quotas

Every guest export crosses a validated, serializable message boundary. The
current important ceilings are:

- 1 MiB per serialized executor request or response.
- 10,000 rendered UI work items.
- 1,024 actions, including lightbox images.
- 10,000 top tabs.
- 10,000,000 Wasmtime fuel units per export.
- 8 MiB hostcall-copy fuel per store.
- 256 MiB linear memory, four memories, eight tables, 100,000 table elements,
  and 32 adapter-internal core instances.

A quota failure makes the instance terminal. The host disables the plugin and
records the package fingerprint for explicit Retry/Reset handling. Code should
page large collections and return compact layouts/actions rather than relying
on these ceilings as normal operating targets.

## Capabilities are host policy

The SDK exposes calls; it does not grant them. `plugin.toml` requests the
capabilities, the installation dialog discloses them, and the host enforces
them at every operation. Missing authority fails closed.

See the [security model](../docs/wirt/security-model.md) and
[ABI policy](../docs/wirt/abi-policy.md) before publishing a plugin.
