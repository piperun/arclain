#!/usr/bin/env python3
"""Build or clean the WASM plugins under plugins/.

Usage:
    python scripts/_plugins.py build    # build every plugin to wasm32-wasip2
    python scripts/_plugins.py clean    # remove .wasm artifacts + cargo clean

WASM crates compile to a file named with underscores (e.g.
`gstreamer_preview.wasm`); we copy that next to the plugin's
Cargo.toml as `<dir-name>.wasm` (e.g. `gstreamer-preview.wasm`)
so the host loader can find it by the same name as the directory.
"""
from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
PLUGINS_DIR = REPO_ROOT / "plugins"
WASM_TARGET = "wasm32-wasip2"


def ensure_target() -> None:
    """Install the WASM target if not already present."""
    installed = subprocess.run(
        ["rustup", "target", "list", "--installed"],
        capture_output=True, text=True,
    )
    if WASM_TARGET not in installed.stdout:
        print(f"  Installing target {WASM_TARGET}...")
        result = subprocess.run(["rustup", "target", "add", WASM_TARGET])
        if result.returncode != 0:
            sys.exit(result.returncode)


def build() -> int:
    """Build all WASM plugins. Returns 0 on full success, 1 if any failed."""
    print("Building WASM plugins...")
    print(f"  Target: {WASM_TARGET}")
    ensure_target()

    failures: list[str] = []
    for plugin_dir in sorted(PLUGINS_DIR.iterdir()):
        cargo_toml = plugin_dir / "Cargo.toml"
        if not plugin_dir.is_dir() or not cargo_toml.exists():
            continue

        name = plugin_dir.name
        print(f"\n  Building {name}...")

        result = subprocess.run(
            ["cargo", "build", "--target", WASM_TARGET, "--release",
             "--target-dir", "."],
            cwd=plugin_dir,
        )
        if result.returncode != 0:
            print(f"  ERROR: Failed to build {name}")
            failures.append(name)
            continue

        wasm_src = plugin_dir / WASM_TARGET / "release" / f"{name.replace('-', '_')}.wasm"
        wasm_dest = plugin_dir / f"{name}.wasm"
        if wasm_src.exists():
            shutil.copy2(wasm_src, wasm_dest)
            print(f"  Built: {name}.wasm ({wasm_dest.stat().st_size:,} bytes)")
        else:
            print(f"  WARNING: WASM file not found at {wasm_src}")
            failures.append(name)

    if failures:
        print(f"\nWARNING: failed plugins: {', '.join(failures)}")
        return 1
    return 0


def clean() -> int:
    """Remove .wasm artifacts and run `cargo clean` per plugin."""
    print("Cleaning WASM plugins...")
    removed = 0
    for wasm in PLUGINS_DIR.rglob("*.wasm"):
        wasm.unlink()
        removed += 1
    print(f"  Removed {removed} .wasm file(s)")

    print("\nRunning cargo clean in each plugin...")
    failures: list[str] = []
    for plugin_dir in sorted(PLUGINS_DIR.iterdir()):
        if not plugin_dir.is_dir() or not (plugin_dir / "Cargo.toml").exists():
            continue
        print(f"  {plugin_dir.name}")
        result = subprocess.run(
            ["cargo", "clean", "-q"], cwd=plugin_dir,
            capture_output=True, text=True,
        )
        if result.returncode != 0:
            print(f"    WARNING: cargo clean failed for {plugin_dir.name}")
            failures.append(plugin_dir.name)

    print("\nClean complete.")
    if failures:
        print(f"WARNING: failed plugin cleans: {', '.join(failures)}")
        return 1
    return 0


def main() -> None:
    if len(sys.argv) < 2 or sys.argv[1] not in ("build", "clean"):
        print("usage: _plugins.py {build|clean}", file=sys.stderr)
        sys.exit(2)
    sys.exit(build() if sys.argv[1] == "build" else clean())


if __name__ == "__main__":
    main()
