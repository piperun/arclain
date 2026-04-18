# Arclain

Desktop app for managing game archives — inspection, batch conversion,
pipeline-based flatten/organize/convert operations, and metadata enrichment
via WASM plugins.

Built in Rust with egui/eframe, runs on Windows / Linux / macOS.

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

## Documentation

### Active / shipping

- [`docs/ARCZIP_FORMAT.md`](docs/ARCZIP_FORMAT.md) —
  **ARCZIP** archive format: standard ZIP with zstd compression and an
  appended PAR2 recovery tail. Opens in any zip tool; arclain surfaces
  verify/repair. This is the format arclain emits when "pack with recovery"
  is selected.
- [`plugins/README.md`](plugins/README.md) — plugin architecture and
  authoring guide.

### Deferred / research

- [`docs/future/care/`](docs/future/care/) — **CARE**, an ambitious
  next-gen archive format. Not on the roadmap; see the folder's README for
  why, and why ARCZIP ships instead.

> Other markdown under `docs/` is local-only scratchpad (per `.gitignore`).

## Conventions

- Conventional commits with package scopes (`feat(ui):`, `fix(core):`, …).
- Monorepo versioning via cocogitto — `cog bump --auto` bumps per-package.
- Rust edition 2021, stable toolchain.

## License

(See `LICENSE` if present.)
