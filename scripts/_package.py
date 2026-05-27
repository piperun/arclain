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
import platform
import shutil
import sys
import tomllib
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
PLUGINS_DIR = REPO_ROOT / "plugins"
SKIP_PLUGINS = {"gstreamer-preview", "ui-demo"}


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


def main() -> None:
    parser = argparse.ArgumentParser(description="Package arclain binary + plugins.")
    parser.add_argument(
        "--profile", required=True, choices=["debug", "release"],
        help="Cargo profile that was used for the build",
    )
    parser.add_argument(
        "--archive", action="store_true",
        help="Zip the output folder and write a sha256 sidecar",
    )
    parser.add_argument(
        "--version", default=None,
        help="Override the version string (default: read from Cargo.toml)",
    )
    args = parser.parse_args()

    version = args.version or workspace_version()
    out_root = REPO_ROOT / args.profile  # release/ or debug/
    label = args.profile

    os_name, arch = get_platform()
    binary_name = "arclain.exe" if os_name == "windows" else "arclain"
    src_binary = "arclain_ui.exe" if os_name == "windows" else "arclain_ui"

    pkg_name = f"arclain-{version}-{os_name}-{arch}"
    pkg_dir = out_root / pkg_name

    print(f"Packaging {label} ({pkg_name})...")

    if pkg_dir.exists():
        shutil.rmtree(pkg_dir)
    pkg_dir.mkdir(parents=True)

    target_dir = REPO_ROOT / "target"
    cargo_profile_dir = "release" if args.profile == "release" else "debug"
    exe_path = target_dir / cargo_profile_dir / src_binary
    if not exe_path.exists():
        print(f"ERROR: Binary not found at {exe_path}")
        print("  Did `cargo build` run first?")
        sys.exit(1)
    shutil.copy2(exe_path, pkg_dir / binary_name)

    plugins_dest = pkg_dir / "plugins"
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

    if args.archive:
        archive_fmt = "zip" if os_name == "windows" else "gztar"
        archive_base = out_root / pkg_name
        archive_path = Path(shutil.make_archive(
            str(archive_base), archive_fmt,
            root_dir=str(out_root),
            base_dir=pkg_name,
        ))
        checksum = sha256_file(archive_path)
        checksum_file = archive_path.with_suffix(archive_path.suffix + ".sha256")
        checksum_file.write_text(f"{checksum}  {archive_path.name}\n")
        print(f"\n=== {label.capitalize()} Package Complete ===")
        print(f"Package:  {archive_path}")
        print(f"Checksum: {checksum_file}")
        print(f"Version:  {version}")
    else:
        print(f"\n=== {label.capitalize()} Build Complete ===")
        print(f"Folder:   {pkg_dir}")
        print(f"Run with: {pkg_dir / binary_name}")


if __name__ == "__main__":
    main()
