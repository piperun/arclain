#!/usr/bin/env python3
"""
Arclain dev + release CLI.

Usage:
    python scripts/release.py release
    python scripts/release.py debug
    python scripts/release.py plugins
    python scripts/release.py ui [-- <cargo run args>]
    python scripts/release.py clean-plugins
    python scripts/release.py deps [--update | --upgrade [--incompatible]] [--dry-run]

`release` produces an optimized platform archive under release/.
Versioning is NOT done here — run `cog bump --minor` (or similar)
locally before tagging; this script assumes the version in
Cargo.toml is already correct for the build.

`debug` skips tests and produces an unoptimized binary + plugins
under debug/ for fast UI iteration; not meant for distribution.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Callable

import _package

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT_DIR = Path(__file__).resolve().parent
PLUGINS_DIR = REPO_ROOT / "plugins"
WASM_TARGET = "wasm32-wasip2"
SKIP_PLUGINS = {"gstreamer-preview", "ui-demo"}
LOGGING_CONFIG = SCRIPT_DIR / "logging_config.json"
DEFAULT_RUST_LOG = "arclain=debug,info"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def run(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
) -> None:
    """Run a command, exit on failure."""
    print(f"  > {' '.join(cmd)}")
    merged_env: dict[str, str] = {**os.environ, **(env or {})}
    result = subprocess.run(cmd, cwd=cwd, env=merged_env)
    if result.returncode != 0:
        sys.exit(result.returncode)


def run_passthrough(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
) -> int:
    """Run a command and return its exit code (don't exit on failure)."""
    print(f"  > {' '.join(cmd)}")
    merged_env: dict[str, str] = {**os.environ, **(env or {})}
    return subprocess.run(cmd, cwd=cwd, env=merged_env).returncode


def get_version_from_cargo() -> str:
    """Extract `[workspace.package].version` from the root Cargo.toml.

    Reads the unified workspace version, not a per-crate file. After
    the workspace.package migration, per-crate Cargo.toml files use
    `version.workspace = true` and contain no explicit version string
    — naively grepping them now picks up a *dependency*'s version
    (e.g. egui_extras = { version = "0.33.0", ... }) and ships
    binaries with completely wrong names. Use tomllib (stdlib since
    Python 3.11) to walk the structured tree instead.
    """
    cargo_toml = REPO_ROOT / "Cargo.toml"
    if not cargo_toml.exists():
        return "0.0.0"

    import tomllib
    with open(cargo_toml, "rb") as f:
        data = tomllib.load(f)

    return (
        data.get("workspace", {})
        .get("package", {})
        .get("version", "0.0.0")
    )


def have_command(name: str) -> bool:
    """True if `name` is on PATH."""
    return shutil.which(name) is not None


# ---------------------------------------------------------------------------
# Plugin build
# ---------------------------------------------------------------------------


def build_plugins() -> bool:
    """Build all WASM plugins. Returns True if all succeeded."""
    print("Building WASM plugins...")
    print(f"  Target: {WASM_TARGET}")

    # Ensure target is installed
    installed = subprocess.run(
        ["rustup", "target", "list", "--installed"],
        capture_output=True, text=True,
    )
    if WASM_TARGET not in installed.stdout:
        print(f"  Installing target {WASM_TARGET}...")
        run(["rustup", "target", "add", WASM_TARGET])

    all_ok = True
    for plugin_dir in sorted(PLUGINS_DIR.iterdir()):
        cargo_toml = plugin_dir / "Cargo.toml"
        if not plugin_dir.is_dir() or not cargo_toml.exists():
            continue

        plugin_name = plugin_dir.name
        print(f"\n  Building {plugin_name}...")

        result = subprocess.run(
            ["cargo", "build", "--target", WASM_TARGET, "--release", "--target-dir", "."],
            cwd=plugin_dir,
        )
        if result.returncode != 0:
            print(f"  ERROR: Failed to build {plugin_name}")
            all_ok = False
            continue

        # WASM filename uses underscores
        wasm_name = plugin_name.replace("-", "_")
        wasm_src = plugin_dir / WASM_TARGET / "release" / f"{wasm_name}.wasm"
        wasm_dest = plugin_dir / f"{plugin_name}.wasm"

        if wasm_src.exists():
            shutil.copy2(wasm_src, wasm_dest)
            size = wasm_dest.stat().st_size
            print(f"  Built: {plugin_name}.wasm ({size:,} bytes)")
        else:
            print(f"  WARNING: WASM file not found at {wasm_src}")

    return all_ok


def clean_plugins() -> None:
    """Remove all .wasm artifacts and `cargo clean` every plugin."""
    print("Cleaning WASM plugins...")

    removed = 0
    for wasm in PLUGINS_DIR.rglob("*.wasm"):
        wasm.unlink()
        removed += 1
    print(f"  Removed {removed} .wasm file(s)")

    print("\nRunning cargo clean in each plugin...")
    for plugin_dir in sorted(PLUGINS_DIR.iterdir()):
        if not plugin_dir.is_dir() or not (plugin_dir / "Cargo.toml").exists():
            continue

        print(f"  {plugin_dir.name}")
        result = subprocess.run(
            ["cargo", "clean", "-q"], cwd=plugin_dir, capture_output=True, text=True,
        )
        if result.returncode != 0:
            print(f"    WARNING: cargo clean failed for {plugin_dir.name}")

    print("\nClean complete.")


# ---------------------------------------------------------------------------
# Logging config -> RUST_LOG
# ---------------------------------------------------------------------------


def load_rust_log() -> str:
    """Assemble RUST_LOG string from logging_config.json, or return default."""
    if not LOGGING_CONFIG.exists():
        print(f"  {LOGGING_CONFIG.name} not found, using default")
        return DEFAULT_RUST_LOG

    try:
        data = json.loads(LOGGING_CONFIG.read_text())
    except (OSError, json.JSONDecodeError) as e:
        print(f"  Failed to parse {LOGGING_CONFIG.name}: {e}")
        return DEFAULT_RUST_LOG

    parts = [data.get("default_level", "info")]
    filters = data.get("filters", {})
    for module, level in filters.items():
        parts.append(f"{module}={level}")
    return ",".join(parts)


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------


def _package_build(
    *,
    profile: str,
    archive_output: bool,
    version: str,
) -> None:
    """Shared build-and-package pipeline used by both `release` and `debug`.

    `profile` is the cargo build profile ("release" or "dev"/"debug"). Plugins
    are always built with --release because debug WASM is enormous and the
    runtime difference doesn't matter for testing host UI changes.

    `archive_output=True` archives the folder for distribution. Debug builds
    skip this since they're not meant for distribution.
    """
    cargo_cmd = ["cargo", "build", "--package", "arclain_ui"]
    if profile == "release":
        cargo_cmd.insert(2, "--release")
    run(cargo_cmd, cwd=REPO_ROOT)

    print("\nBuilding plugins...")
    if not build_plugins():
        print("ERROR: Some plugins failed to build.")
        sys.exit(1)

    _package.package(
        profile=profile,
        archive=archive_output,
        version=version,
    )


def cmd_release(args: argparse.Namespace) -> None:
    """Full release workflow: optimized build, plugins, platform archive.

    Versioning happens separately via `cog bump` (run locally before
    tagging) — this script doesn't touch versions. CI invokes us
    after the tag is already on the remote.
    """
    _ = args
    print("=== Arclain Release Build ===")
    print(f"Repository: {REPO_ROOT}")

    target_dir = _package.cargo_target_dir(REPO_ROOT)
    print(f"Using Cargo target directory: {target_dir}\n")

    version = get_version_from_cargo()
    print(f"Building version: {version}\n")

    print("Building optimized binary + plugins...")
    _package_build(
        profile="release",
        archive_output=True,
        version=version,
    )


def cmd_debug(_args: argparse.Namespace) -> None:
    """Fast iteration build: debug profile, with plugins, no zip, no version
    bump, no test suite. Lands under `debug/` so release artifacts stay
    untouched.

    Cuts host compile time from ~3min to ~30-60s for iteration. Use when
    you want to test a UI change end-to-end with plugins loaded but
    don't need an optimized binary.
    """
    print("=== Arclain Debug Build (fast iteration) ===")
    print(f"Repository: {REPO_ROOT}")

    target_dir = _package.cargo_target_dir(REPO_ROOT)
    print(f"Using Cargo target directory: {target_dir}\n")

    version = get_version_from_cargo()
    print(f"Building version: {version}\n")

    print("Building debug binary + plugins...")
    _package_build(
        profile="debug",
        archive_output=False,
        version=version,
    )


def cmd_plugins(_args: argparse.Namespace) -> None:
    """Standalone plugin build."""
    if not build_plugins():
        print("\nWARNING: Some plugins failed to build.")
        sys.exit(1)

    # Summary
    print("\nPlugin files:")
    for plugin_dir in sorted(PLUGINS_DIR.iterdir()):
        wasm = plugin_dir / f"{plugin_dir.name}.wasm"
        if wasm.exists():
            size = wasm.stat().st_size
            print(f"  {wasm.relative_to(REPO_ROOT)} ({size:,} bytes)")


def cmd_clean_plugins(_args: argparse.Namespace) -> None:
    """Remove .wasm artifacts and clean each plugin's build dir."""
    _ = _args
    clean_plugins()


def cmd_ui(args: argparse.Namespace) -> None:
    """Launch `cargo ui` with RUST_LOG populated from logging_config.json."""
    rust_log = load_rust_log()
    print(f"RUST_LOG = {rust_log}")

    env: dict[str, str] = {"RUST_LOG": rust_log, "CARGO_TERM_COLOR": "always"}
    # Forward any extra args after `ui` to `cargo ui`
    extra: list[str] = list(args.cargo_args)
    cargo_cmd: list[str] = ["cargo", "ui", *extra]
    sys.exit(run_passthrough(cargo_cmd, cwd=REPO_ROOT, env=env))


def cmd_deps(args: argparse.Namespace) -> None:
    """Inspect or update cargo dependencies."""
    print("=== Cargo Dependency Manager ===")
    print(f"Workspace: {REPO_ROOT}\n")

    if args.upgrade:
        if not have_command("cargo-upgrade"):
            print("cargo-edit is required for --upgrade.")
            print("  Install it:  cargo install cargo-edit\n")
            print("Or use --update to just update Cargo.lock (no cargo-edit needed).")
            sys.exit(1)

        cmd = ["cargo", "upgrade", "--workspace"]
        if args.incompatible:
            cmd.append("--incompatible")
            print("(Including incompatible/breaking version updates)")
        if args.dry_run:
            cmd.append("--dry-run")
            print("(Dry run - no changes will be made)")

        sys.exit(run_passthrough(cmd, cwd=REPO_ROOT))

    if args.update:
        cmd = ["cargo", "update"]
        if args.dry_run:
            cmd.append("--dry-run")
            print("(Dry run - no changes will be made)")
        sys.exit(run_passthrough(cmd, cwd=REPO_ROOT))

    # Default: check for outdated
    if have_command("cargo-outdated"):
        print("Using cargo-outdated:")
        sys.exit(run_passthrough(["cargo", "outdated", "--workspace"], cwd=REPO_ROOT))

    print("Note: install 'cargo-outdated' for a detailed report:")
    print("  cargo install cargo-outdated\n")
    print("Falling back to 'cargo update --dry-run':")
    sys.exit(run_passthrough(["cargo", "update", "--dry-run"], cwd=REPO_ROOT))


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(description="Arclain dev & release tool")
    sub = parser.add_subparsers(dest="command", required=True)

    release_parser = sub.add_parser("release", help="Full release workflow")
    release_parser.add_argument(
        "-t", "--skip-tests", action="store_true",
        help="Deprecated no-op; releases no longer run tests implicitly",
    )

    sub.add_parser(
        "debug",
        help=(
            "Fast iteration build: debug profile, with plugins, no zip, no "
            "version bump, no tests. Output goes under debug/ so release "
            "artifacts stay untouched. Use during UI iteration; switch back "
            "to `release` when shipping."
        ),
    )

    sub.add_parser("plugins", help="Build WASM plugins")
    sub.add_parser("clean-plugins", help="Remove .wasm artifacts and cargo clean each plugin")

    ui_parser = sub.add_parser("ui", help="Run `cargo ui` with RUST_LOG from logging_config.json")
    ui_parser.add_argument(
        "cargo_args", nargs=argparse.REMAINDER,
        help="Extra args forwarded to `cargo ui`",
    )

    deps_parser = sub.add_parser("deps", help="Check or update cargo dependencies")
    mode = deps_parser.add_mutually_exclusive_group()
    mode.add_argument("--update", action="store_true", help="Run `cargo update` (Cargo.lock)")
    mode.add_argument(
        "--upgrade", action="store_true",
        help="Run `cargo upgrade --workspace` (Cargo.toml constraints; needs cargo-edit)",
    )
    deps_parser.add_argument(
        "--incompatible", action="store_true",
        help="With --upgrade, also bump to incompatible/breaking versions",
    )
    deps_parser.add_argument(
        "--dry-run", action="store_true", help="Show what would change without writing",
    )

    args = parser.parse_args()

    dispatch: dict[str, Callable[[argparse.Namespace], None]] = {
        "release": cmd_release,
        "debug": cmd_debug,
        "plugins": cmd_plugins,
        "clean-plugins": cmd_clean_plugins,
        "ui": cmd_ui,
        "deps": cmd_deps,
    }
    dispatch[args.command](args)


if __name__ == "__main__":
    main()
