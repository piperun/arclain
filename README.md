<pre>
                                    ·
                       ▒▓▓██████████▓▓▒
                    ▓██░               ░██▓
                  ▓█░    ▒░░░░░░░░░▒    ░█▓
                 █░     █░  ░░░░░  ░█     ░█
                █     ▒█  ░░ ◉ ░░  █▒     █
                 █░    ░█░  ░░░  ░█░    ░█
                  ▓█░    ▒░░░░░░░░░▒    ░█▓
                    ▓██░               ░██▓
                       ▒▓▓███░█   █░███▓▓▒



               A   R   C   L   A   I   N

          [ 繋がっていたもの ]    ·    what was once connected
</pre>

File archive manager with game-mod archive support, primarily targeting
the [Fluffy](https://www.fluffyquack.com/) mod manager layout.

Uses the `7z` and `unrar` command-line executables for most operations.
Native Rust backends for both formats exist, but the CLI paths handle
most real-world work.

Mainly used for:

- Format conversion
- Batch operations
- Flatten / standardize archives via pre-defined rules

Comes with a **DLSite metadata plugin**.

Built in Rust with egui/eframe. Runs on Windows / Linux / macOS.

> [!WARNING]
> Only tested on Windows.
> Requires `7z` and `unrar` executables on `PATH`.


## Quick start

```bash
# Run the UI
python scripts/release.py ui

# Build WASM plugins
python scripts/release.py plugins

# Full release build
python scripts/release.py release
```

See [`scripts/release.py --help`](scripts/release.py) for all subcommands
(`ui`, `plugins`, `clean-plugins`, `deps`, `release`).

## Workspace

| Crate | Purpose |
|-------|---------|
| `crates/core` | Archive backends, pipelines, organization rules, plugin host |
| `crates/ui` | egui frontend (primary binary) |
| `crates/db` | SQLite persistence layer (archives, presets, rules) |
| `crates/plugins` | WASM plugin runtime (wasmtime-based) |
| `crates/widgets` | Reusable themed UI components |
| `crates/theme` | Theme + color tokens |
| `crates/network` | HTTP client with proxy support |
| `crates/data` | Shared data types |
| `crates/signals` | Reactive state primitives |
| `crates/checksum` | CRC32/Blake3 helpers |
| `plugins/*` | WASM plugins (dlsite-metadata, gstreamer-preview, …) |


## Conventions

- Conventional commits with package scopes (`feat(ui):`, `fix(core):`, …).
- Monorepo versioning via cocogitto 7 — workspace crates share a single
  unified version, bumped with `cog bump --auto` (or `--minor` / `--patch`).
  The `dlsite-metadata` plugin ships out-of-band and bumps independently
  via `cog bump --package dlsite-metadata`.
- Rust edition 2021, stable toolchain.

## License

GPL-3.0-or-later. See [`LICENSE`](LICENSE) for the full text.
