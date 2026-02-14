#!/usr/bin/env python3
"""
Arclain Release & Build Script

Usage:
    python scripts/release.py release [--skip-version-update] [--skip-tests]
    python scripts/release.py plugins
"""

from __future__ import annotations

import argparse
import hashlib
import os
import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PLUGINS_DIR = REPO_ROOT / "plugins"
WASM_TARGET = "wasm32-wasip2"
SKIP_PLUGINS = {"gstreamer-preview", "ui-demo"}


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def run(cmd: list[str], *, cwd: Path | None = None, env: dict | None = None) -> None:
    """Run a command, exit on failure."""
    print(f"  > {' '.join(cmd)}")
    merged_env = {**os.environ, **(env or {})}
    result = subprocess.run(cmd, cwd=cwd, env=merged_env)
    if result.returncode != 0:
        sys.exit(result.returncode)


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


# ---------------------------------------------------------------------------
# Release
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
            ["cargo", "test", "--workspace", "--", "--test-threads=1"],
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
    archive_ext = ".zip" if os_name == "windows" else ".tar.gz"
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


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(description="Arclain build & release tool")
    sub = parser.add_subparsers(dest="command", required=True)

    release_parser = sub.add_parser("release", help="Full release workflow")
    release_parser.add_argument(
        "--skip-version-update", action="store_true",
        help="Skip the cocogitto version bump step",
    )
    release_parser.add_argument(
        "--skip-tests", action="store_true",
        help="Skip the test suite (use for hotfixes only)",
    )

    sub.add_parser("plugins", help="Build WASM plugins only")

    args = parser.parse_args()

    if args.command == "release":
        cmd_release(args)
    elif args.command == "plugins":
        cmd_plugins(args)


if __name__ == "__main__":
    main()
