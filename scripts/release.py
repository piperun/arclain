#!/usr/bin/env python3
"""
Arclain dev + release CLI.

Usage:
    python scripts/release.py release [--skip-version-update] [--skip-tests]
    python scripts/release.py plugins
    python scripts/release.py ui [-- <cargo run args>]
    python scripts/release.py clean-plugins
    python scripts/release.py deps [--update | --upgrade [--incompatible]] [--dry-run]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Callable

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
    """Extract version from crates/ui/Cargo.toml."""
    cargo_toml = REPO_ROOT / "crates" / "ui" / "Cargo.toml"
    if not cargo_toml.exists():
        return "0.0.0"

    content = cargo_toml.read_text()
    match = re.search(r'version\s*=\s*"([^"]+)"', content)
    return match.group(1) if match else "0.0.0"


def get_platform() -> tuple[str, str]:
    """Returns (os_name, arch) for the current platform."""
    system = platform.system().lower()
    machine = platform.machine().lower()

    if system == "darwin":
        os_name = "macos"
    elif system == "windows":
        os_name = "windows"
    else:
        os_name = "linux"

    if machine in ("x86_64", "amd64"):
        arch = "x64"
    elif machine in ("aarch64", "arm64"):
        arch = "arm64"
    else:
        arch = machine

    return os_name, arch


def sha256_file(filepath: Path) -> str:
    """Calculate SHA256 hash of a file."""
    sha256 = hashlib.sha256()
    with open(filepath, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            sha256.update(chunk)
    return sha256.hexdigest()


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


def cmd_release(args: argparse.Namespace) -> None:
    """Full release workflow."""
    print("=== Arclain Release Build ===")
    print(f"Repository: {REPO_ROOT}")

    # Use project-local target dir for release builds (not ramdisk)
    target_dir = REPO_ROOT / "target"
    release_env = {"CARGO_TARGET_DIR": str(target_dir)}
    print(f"Using project target directory: {target_dir}\n")

    # Step 1: Version bump
    if not args.skip_version_update:
        print("Step 1: Bumping crate versions (cog)...")
        result = subprocess.run(
            ["cog", "bump", "--auto", "--skip-untracked"],
            capture_output=True, text=True, cwd=REPO_ROOT,
        )
        if result.returncode != 0:
            if "No conventional commit found" in (result.stdout + result.stderr):
                print("  No version bumps needed")
            else:
                print(result.stdout)
                print(result.stderr)
                print("Error: Version bump failed")
                sys.exit(1)
        else:
            if result.stdout.strip():
                print(result.stdout.strip())
            print("  Version bump complete")
    else:
        print("Step 1: Skipping version update")

    # Read version for package naming
    version = get_version_from_cargo()
    print(f"Building version: {version}\n")

    # Step 2: Tests
    if not args.skip_tests:
        print("Step 2: Running test suite...")
        run(
            ["cargo", "test", "--workspace"],
            cwd=REPO_ROOT, env=release_env,
        )
        print("All tests passed!\n")
    else:
        print("Step 2: Skipping tests\n")

    # Step 3: Build release binary
    print("Step 3: Building release binary...")
    run(
        ["cargo", "build", "--release", "--package", "arclain_ui"],
        cwd=REPO_ROOT, env=release_env,
    )

    # Build plugins
    print("\nBuilding plugins...")
    if not build_plugins():
        print("WARNING: Some plugins failed to build, continuing...")

    # Step 4: Package
    os_name, arch = get_platform()
    binary_name = "arclain.exe" if os_name == "windows" else "arclain"
    src_binary = "arclain_ui.exe" if os_name == "windows" else "arclain_ui"

    release_name = f"arclain-{version}-{os_name}-{arch}"
    release_dir = REPO_ROOT / "release" / release_name

    print(f"\nStep 4: Packaging release ({release_name})...")

    # Clean and create
    if release_dir.exists():
        shutil.rmtree(release_dir)
    release_dir.mkdir(parents=True)

    # Copy binary
    exe_path = target_dir / "release" / src_binary
    if not exe_path.exists():
        print(f"Error: Binary not found at {exe_path}")
        sys.exit(1)
    shutil.copy2(exe_path, release_dir / binary_name)

    # Copy plugins
    plugins_dest = release_dir / "plugins"
    plugins_dest.mkdir()

    for plugin_dir in sorted(PLUGINS_DIR.iterdir()):
        if not plugin_dir.is_dir():
            continue
        name = plugin_dir.name
        if name in SKIP_PLUGINS:
            print(f"  Skipping unused plugin: {name}")
            continue

        wasm = plugin_dir / f"{name}.wasm"
        if wasm.exists():
            shutil.copy2(wasm, plugins_dest)
            print(f"  Copied plugin: {name}.wasm")

        toml = plugin_dir / f"{name}.toml"
        if toml.exists():
            shutil.copy2(toml, plugins_dest)

    # Create archive
    archive_fmt = "zip" if os_name == "windows" else "gztar"
    archive_base = REPO_ROOT / "release" / release_name

    archive_path = Path(shutil.make_archive(
        str(archive_base), archive_fmt,
        root_dir=str(REPO_ROOT / "release"),
        base_dir=release_name,
    ))

    # Checksum
    checksum = sha256_file(archive_path)
    checksum_file = archive_path.with_suffix(archive_path.suffix + ".sha256")
    checksum_file.write_text(f"{checksum}  {archive_path.name}\n")

    print(f"\n=== Release Complete ===")
    print(f"Package:  {archive_path}")
    print(f"Checksum: {checksum_file}")
    print(f"Version:  {version}")


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
        "-v", "--skip-version-update", action="store_true",
        help="Skip the cocogitto version bump step",
    )
    release_parser.add_argument(
        "-t", "--skip-tests", action="store_true",
        help="Skip the test suite (use for hotfixes only)",
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
        "plugins": cmd_plugins,
        "clean-plugins": cmd_clean_plugins,
        "ui": cmd_ui,
        "deps": cmd_deps,
    }
    dispatch[args.command](args)


if __name__ == "__main__":
    main()
