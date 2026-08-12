# Developing Wirt plugins

Wirt is Arclain's product-neutral plugin API, SDK, and ABI. A Wirt plugin is a
WebAssembly Component packaged as one validated `.wirt` file. Wirt is the
boundary between a host product and a plugin; Wirt itself is not a plugin.

## Quick start

Install the Rust target once. The repository never installs toolchains or
targets automatically:

```console
rustup target add wasm32-wasip2
```

Then create, build, validate, package, and validate a starter project:

```console
just wirt new plugins/my-plugin
just wirt build plugins/my-plugin
just wirt validate plugins/my-plugin
just wirt package plugins/my-plugin
just wirt validate plugins/my-plugin/my-plugin-0.1.0.wirt
```

`wirt new` copies the maintained starter and a local `wirt-sdk` into an empty
destination. Edit the starter metadata in `Cargo.toml`, `plugin.toml`, and
`src/lib.rs` together before publishing.

Install the resulting `.wirt` file from Arclain's Plugins page. Arclain first
shows the package identity, exact Wirt ABI, requested capabilities, network
domains, and SHA-256 fingerprint. Installation happens only after explicit
approval. Loose component files are not accepted by the public install flow.

## Project layout

```text
my-plugin/
├── Cargo.toml
├── Cargo.lock
├── plugin.toml
├── src/
│   └── lib.rs
└── wirt-sdk/              # vendored by `wirt new`
```

The generated `<id>-<version>.wirt` is a distribution artifact and is ignored
by Git. The package contains the manifest and component; it is the only format
accepted by the public install flow.

Repository-maintained projects use `wirt-sdk = { path = "../../wirt-sdk" }`
instead of the vendored path. Every maintained project also contains an empty
`[workspace]` table so it builds as a standalone workspace from nested Git
worktrees.

## Manifest

Every project has one canonical `plugin.toml`:

```toml
[wirt]
abi = "0.3.0"

[plugin]
id = "my-plugin"
name = "My Plugin"
version = "0.1.0"
author = "Your Name"
description = "What the plugin does"

[capabilities]
network = false
network_domains = []
archive_metadata_read = true
archive_metadata_write = false
archive_modify = false
file_read = false
file_write = false

[rate_limits]
http_requests_per_minute = 10
```

Request only the authority the plugin needs:

- `network` permits host-mediated HTTP only to exact approved domains.
- `archive_metadata_read` permits archive context and metadata reads.
- `archive_metadata_write` permits bounded metadata publication.
- `archive_modify` permits supported changes to the current archive.
- `file_read` permits bounded reads from approved host data sources.
- `file_write` permits private cache writes and private temporary files.

Setting `network = true` requires at least one `network_domains` entry. A
plugin cannot widen these grants at runtime.

## Guest implementation

Implement the generated `Guest` trait and export the component world:

```rust
struct Component;

impl wirt_sdk::Guest for Component {
    fn get_metadata() -> wirt_sdk::wirt::plugin::meta::PluginMetadata {
        wirt_sdk::wirt::plugin::meta::PluginMetadata {
            id: "my-plugin".to_string(),
            name: "My Plugin".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            author: "Your Name".to_string(),
            description: "What the plugin does".to_string(),
        }
    }

    fn init() {
        wirt_sdk::info("My plugin initialized");
    }

    fn get_default_rules() -> Vec<wirt_sdk::wirt::plugin::rules::PluginRuleDefinition> {
        vec![]
    }

    fn get_ui_layout(_: String) -> wirt_sdk::wirt::plugin::ui::PluginLayout {
        wirt_sdk::wirt::plugin::ui::PluginLayout::Single(vec![])
    }

    fn on_ui_event(
        _: String,
        _: Option<String>,
    ) -> Vec<wirt_sdk::wirt::plugin::ui::PluginAction> {
        vec![]
    }

    fn get_top_tabs() -> Vec<wirt_sdk::wirt::plugin::ui::TopTabConfig> {
        vec![]
    }
}

wirt_sdk::export!(Component with_types_in wirt_sdk);
```

The values returned by `get_metadata` must exactly match all five identity
fields in `plugin.toml`.

## Repository builds

Build and validate every maintained project:

```console
just plugins
```

This leaves one `<id>-<version>.wirt` beside each project's `Cargo.toml` and
removes generated loose components. It fails if the Rust target is absent, a
manifest is invalid, packaging fails, or validation rejects the result.

Remove generated archives/components and clean each standalone project:

```console
just clean-plugins
```

Release assembly validates each shipping package again and copies `.wirt`
files only. Loose manifests and components never enter the release plugin
directory.

## Debugging

- Use `wirt_sdk::{debug, info, warn, error}` for bounded plugin logging.
- Run `just wirt validate <project-or-package>` before investigating host
  loading failures.
- Check that `plugin.toml` and `get_metadata` agree exactly.
- Check that every host operation has its corresponding manifest capability.
- Treat a resource-blocked state as a real quota violation. One explicit
  retry is allowed at a time; repeated failed retries can persistently disable
  the exact package fingerprint until Reset.

## More detail

- [Wirt SDK](../wirt-sdk/README.md)
- [ABI policy](../docs/wirt/abi-policy.md)
- [Package format](../docs/wirt/package-format.md)
- [Security model](../docs/wirt/security-model.md)
