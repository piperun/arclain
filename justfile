# Arclain dev & release tasks. Run `just --list` to see all recipes.
#
# Cross-platform: works on Windows (PowerShell) and Linux/macOS (sh).
# Heavy logic lives in scripts/_*.py — keeps recipes readable and
# argparse-free dispatch out of the way.

set windows-shell := ["powershell.exe", "-NoProfile", "-Command"]

python := if os_family() == "windows" { "python" } else { "python3" }

# Default when you type bare `just`: fast iteration build.
default: debug

# Fast iteration: debug binary + plugins, no zip, no tests.
debug:
    cargo build -p arclain_ui
    just plugins
    {{python}} scripts/_package.py --profile debug

# `skip-tests=true` skips `cargo test --workspace` (hotfixes only).

# Full release: tests, optimized binary, plugins, zip + sha256.
release skip-tests="false":
    {{ if skip-tests == "true" { "echo 'Skipping tests'" } else { "cargo test --workspace" } }}
    cargo build --release -p arclain_ui
    just plugins
    {{python}} scripts/_package.py --profile release --archive

# Build WASM plugins for all crates under plugins/.
plugins:
    {{python}} scripts/_plugins.py build

# Remove .wasm artifacts and cargo clean each plugin.
clean-plugins:
    {{python}} scripts/_plugins.py clean

# Extra args forward verbatim: `just ui --features dev-foo`.

# Run `cargo ui` with RUST_LOG from scripts/logging_config.json.
ui *args:
    {{python}} scripts/_ui.py {{args}}

# `just deps`                          — outdated check
# `just deps --update`                 — bump Cargo.lock
# `just deps --upgrade`                — bump Cargo.toml (needs cargo-edit)
# `just deps --upgrade --incompatible` — include breaking version bumps
# Add `--dry-run` to any to preview.

# Cargo dependency tools (outdated / update / upgrade).
deps *args:
    {{python}} scripts/_deps.py {{args}}

# ─── cocogitto ──────────────────────────────────────────────────────────
# Tag format is controlled by `cog.toml` (cog 7 default = unprefixed
# `{version}`; the old `-A "v{{version}}"` annotation flag from cog 6
# is gone and does nothing useful in cog 7). All bump recipes accept
# extra args, e.g. `just bump --dry-run`.

# Verify commit messages on the current branch.
cog-check:
    cog check

# Generate CHANGELOG.md from conventional commits.
changelog:
    cog changelog

# Auto-detect bump type from conventional commits; create the tag.
bump *args:
    cog bump --auto {{args}}

# Explicit major bump.
bump-major *args:
    cog bump --major {{args}}

# Explicit minor bump.
bump-minor *args:
    cog bump --minor {{args}}

# Explicit patch bump.
bump-patch *args:
    cog bump --patch {{args}}

# Workspace bump using the `release` hook profile (cargo test + check).
bump-release *args:
    cog bump --auto --hook-profile release {{args}}

# Bump a specific monorepo package: `just bump-package dlsite-metadata`.
bump-package name *args:
    cog bump --package {{name}} --auto {{args}}
