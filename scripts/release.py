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
import _plugins

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT_DIR = Path(__file__).resolve().parent
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


def have_command(name: str) -> bool:
    """True if `name` is on PATH."""
    return shutil.which(name) is not None


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
    plugin_status = _plugins.build()
    if plugin_status != 0:
        print("ERROR: Some plugins failed to build.")
        sys.exit(plugin_status)

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

    version = _package.workspace_version()
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

    version = _package.workspace_version()
    print(f"Building version: {version}\n")

    print("Building debug binary + plugins...")
    _package_build(
        profile="debug",
        archive_output=False,
        version=version,
    )


def cmd_plugins(_args: argparse.Namespace) -> None:
    """Standalone plugin build."""
    plugin_status = _plugins.build()
    if plugin_status != 0:
        print("\nWARNING: Some plugins failed to build.")
        sys.exit(plugin_status)


def cmd_clean_plugins(_args: argparse.Namespace) -> None:
    """Remove .wasm artifacts and clean each plugin's build dir."""
    plugin_status = _plugins.clean()
    if plugin_status != 0:
        sys.exit(plugin_status)


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

    sub.add_parser("plugins", help="Build validated Wirt plugin archives")
    sub.add_parser("clean-plugins", help="Remove Wirt artifacts and clean each plugin")

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
