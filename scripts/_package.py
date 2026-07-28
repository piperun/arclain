#!/usr/bin/env python3
"""Package the built binary + plugin .wasm files into a release folder.

Usage:
    python scripts/_package.py --profile {debug|release}
    python scripts/_package.py --profile release --archive
    python scripts/_package.py --profile release --version 1.2.3 --archive

The output goes under `release/` for release profile and `debug/` for
debug profile, in a folder named `arclain-<version>-<os>-<arch>/`.
With --archive, also create a zip (Windows) or tar.gz (Linux/macOS)
and a sha256 sidecar.

The cargo build itself is NOT done here — the justfile invokes
`cargo build [-p arclain_ui] [--release]` directly, then calls us
to assemble + package. This keeps each piece focused.

Version defaults to `[workspace.package].version` from the root
Cargo.toml. Naive grepping picks up dependency versions instead,
which is why we parse the structured TOML.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
PLUGINS_DIR = REPO_ROOT / "plugins"
# "facade-test-fixture" is arclain_app's own dedicated test plugin (crash
# containment / action ordering / refresh coalescing fixtures for
# crates/app/tests/plugin_sessions.rs -- see plugins/facade-test-fixture/
# src/lib.rs) -- never user-facing, and must never reach a release package.
SKIP_PLUGINS = {"facade-test-fixture", "gstreamer-preview", "ui-demo"}


def get_platform() -> tuple[str, str]:
    """Return (os_name, arch) using arclain's naming convention."""
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
    sha256 = hashlib.sha256()
    with open(filepath, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            sha256.update(chunk)
    return sha256.hexdigest()


def workspace_version() -> str:
    """Read `[workspace.package].version` from root Cargo.toml."""
    cargo_toml = REPO_ROOT / "Cargo.toml"
    if not cargo_toml.exists():
        return "0.0.0"
    with open(cargo_toml, "rb") as f:
        data = tomllib.load(f)
    return (
        data.get("workspace", {})
        .get("package", {})
        .get("version", "0.0.0")
    )


def cargo_target_dir(repo_root: Path = REPO_ROOT) -> Path:
    """Return the Cargo target directory for this workspace."""
    env_target = os.environ.get("CARGO_TARGET_DIR")
    if env_target:
        target_dir = Path(env_target)
        if not target_dir.is_absolute():
            target_dir = repo_root / target_dir
        return target_dir

    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=repo_root,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print("ERROR: Failed to determine Cargo target directory.", file=sys.stderr)
        if result.stderr.strip():
            print(result.stderr.strip(), file=sys.stderr)
        raise SystemExit(result.returncode)

    try:
        metadata = json.loads(result.stdout)
        return Path(metadata["target_directory"])
    except (json.JSONDecodeError, KeyError) as error:
        print("ERROR: Cargo metadata did not report target_directory.", file=sys.stderr)
        print(f"  {error}", file=sys.stderr)
        raise SystemExit(1) from error


def package_source_inputs(repo_root: Path = REPO_ROOT) -> list[Path]:
    """Return source inputs that should be older than the packaged binary."""
    files: list[Path] = []
    for root in (repo_root / "crates", repo_root / "wit"):
        if not root.exists():
            continue
        for pattern in ("*.rs", "*.toml", "*.wit"):
            files.extend(path for path in root.rglob(pattern) if path.is_file())

    for path in (repo_root / "Cargo.toml", repo_root / "Cargo.lock"):
        if path.exists():
            files.append(path)

    return files


def ensure_binary_fresh(binary_path: Path, source_paths: list[Path]) -> None:
    """Fail when the packaged binary predates any host source input."""
    source_files = [path for path in source_paths if path.exists() and path.is_file()]
    if not source_files:
        return

    newest_source = max(source_files, key=lambda path: path.stat().st_mtime)
    if binary_path.stat().st_mtime >= newest_source.stat().st_mtime:
        return

    print("ERROR: Built binary is older than the source tree.", file=sys.stderr)
    print(f"  Binary: {binary_path}", file=sys.stderr)
    print(f"  Newer source: {newest_source}", file=sys.stderr)
    print(
        "  Run `cargo build -p arclain_ui --release` with the same "
        "Cargo target directory first.",
        file=sys.stderr,
    )
    raise SystemExit(1)


def copy_bundled_plugins(
    plugins_dest: Path,
    plugins_dir: Path = PLUGINS_DIR,
) -> list[str]:
    """Copy shippable plugin sidecars into a package plugins directory.

    Every non-skipped plugin crate must provide both `<name>.toml` and
    `<name>.wasm`. Missing sidecars are fatal so release packages cannot
    silently ship without the plugin files the host was tested against.
    """
    plugins_dest.mkdir(parents=True, exist_ok=True)

    copied: list[str] = []
    errors: list[str] = []
    for plugin_dir in sorted(plugins_dir.iterdir()):
        if not plugin_dir.is_dir() or not (plugin_dir / "Cargo.toml").exists():
            continue

        name = plugin_dir.name
        if name in SKIP_PLUGINS:
            print(f"  Skipping unused plugin: {name}")
            continue

        wasm = plugin_dir / f"{name}.wasm"
        toml = plugin_dir / f"{name}.toml"
        missing = [path.name for path in (toml, wasm) if not path.exists()]
        if missing:
            errors.append(f"{name}: missing {', '.join(missing)}")
            continue

        shutil.copy2(toml, plugins_dest)
        shutil.copy2(wasm, plugins_dest)
        copied.append(name)
        print(f"  Copied plugin: {name}.toml")
        print(f"  Copied plugin: {name}.wasm")

    if errors:
        print("ERROR: bundled plugin sidecars are incomplete:", file=sys.stderr)
        for error in errors:
            print(f"  {error}", file=sys.stderr)
        raise SystemExit(1)

    return copied


def create_archive(
    package_dir: Path,
    out_root: Path,
    os_name: str,
) -> tuple[Path, Path]:
    """Create one platform archive and its SHA-256 sidecar."""
    archive_format = "zip" if os_name == "windows" else "gztar"
    archive_suffix = ".zip" if os_name == "windows" else ".tar.gz"
    archive_base = out_root / package_dir.name
    archive_path = out_root / f"{package_dir.name}{archive_suffix}"
    checksum_path = archive_path.with_suffix(archive_path.suffix + ".sha256")

    out_root.mkdir(parents=True, exist_ok=True)
    archive_path.unlink(missing_ok=True)
    checksum_path.unlink(missing_ok=True)

    archive_path = Path(
        shutil.make_archive(
            str(archive_base),
            archive_format,
            root_dir=str(package_dir.parent),
            base_dir=package_dir.name,
        ),
    )
    checksum = sha256_file(archive_path)
    checksum_path.write_text(
        f"{checksum}  {archive_path.name}\n",
        encoding="utf-8",
    )
    return archive_path, checksum_path


def package(
    profile: str,
    archive: bool,
    version: str | None = None,
) -> Path:
    """Assemble a built profile and optionally create its release archive."""
    version = version or workspace_version()
    out_root = REPO_ROOT / profile
    os_name, arch = get_platform()
    binary_name = "arclain.exe" if os_name == "windows" else "arclain"
    src_binary = "arclain_ui.exe" if os_name == "windows" else "arclain_ui"

    pkg_name = f"arclain-{version}-{os_name}-{arch}"
    pkg_dir = out_root / pkg_name

    print(f"Packaging {profile} ({pkg_name})...")

    target_dir = cargo_target_dir()
    cargo_profile_dir = "release" if profile == "release" else "debug"
    exe_path = target_dir / cargo_profile_dir / src_binary
    if not exe_path.exists():
        print(f"ERROR: Binary not found at {exe_path}")
        print("  Did `cargo build` run first with the same Cargo target directory?")
        sys.exit(1)
    ensure_binary_fresh(exe_path, package_source_inputs())

    if pkg_dir.exists():
        shutil.rmtree(pkg_dir)
    pkg_dir.mkdir(parents=True)
    shutil.copy2(exe_path, pkg_dir / binary_name)

    copy_bundled_plugins(pkg_dir / "plugins")

    if archive:
        archive_path, checksum_path = create_archive(pkg_dir, out_root, os_name)
        print(f"\n=== {profile.capitalize()} Package Complete ===")
        print(f"Package:  {archive_path}")
        print(f"Checksum: {checksum_path}")
        print(f"Version:  {version}")
        return archive_path

    print(f"\n=== {profile.capitalize()} Build Complete ===")
    print(f"Folder:   {pkg_dir}")
    print(f"Run with: {pkg_dir / binary_name}")
    return pkg_dir


def main() -> None:
    parser = argparse.ArgumentParser(description="Package arclain binary + plugins.")
    parser.add_argument(
        "--profile", required=True, choices=["debug", "release"],
        help="Cargo profile that was used for the build",
    )
    parser.add_argument(
        "--archive", action="store_true",
        help="Archive the output folder and write a sha256 sidecar",
    )
    parser.add_argument(
        "--version", default=None,
        help="Override the version string (default: read from Cargo.toml)",
    )
    args = parser.parse_args()

    package(profile=args.profile, archive=args.archive, version=args.version)


if __name__ == "__main__":
    main()
