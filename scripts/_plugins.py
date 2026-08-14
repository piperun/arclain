#!/usr/bin/env python3
"""Build or clean the Wirt plugins under plugins/.

Usage:
    python scripts/_plugins.py build    # build and validate every .wirt archive
    python scripts/_plugins.py clean    # remove generated archives + cargo clean
"""
from __future__ import annotations

import os
import subprocess
import sys
import tomllib
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
PLUGINS_DIR = REPO_ROOT / "plugins"
WASM_TARGET = "wasm32-wasip2"
with (REPO_ROOT / "wirt-toolchain.toml").open("rb") as handle:
    WIRT_TOOLCHAIN = tomllib.load(handle)["wirt"]
WIRT_REV = WIRT_TOOLCHAIN["rev"]
WIRT_CLI_VERSION = WIRT_TOOLCHAIN["cli_version"]
WIRT_ABI = WIRT_TOOLCHAIN["abi"]
WIRT_COMMAND = [os.environ.get("WIRT", "wirt")]
PRESERVED_ROOT_WASM = {"facade-test-fixture.wasm"}


def ensure_target() -> bool:
    """Return whether the WASM target is installed, without changing Rust."""
    installed = subprocess.run(
        ["rustup", "target", "list", "--installed"],
        capture_output=True, text=True,
    )
    targets = {line.strip() for line in installed.stdout.splitlines()}
    if installed.returncode == 0 and WASM_TARGET in targets:
        return True

    print(f"ERROR: Rust target {WASM_TARGET} is not installed.", file=sys.stderr)
    print(f"  Run `rustup target add {WASM_TARGET}` manually.", file=sys.stderr)
    return False


def _run_wirt(*args: str | Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*WIRT_COMMAND, *(str(arg) for arg in args)],
        cwd=REPO_ROOT,
    )


def verify_wirt_cli() -> bool:
    """Return whether the configured Wirt executable has the reviewed identity."""
    expected = f"wirt-cli {WIRT_CLI_VERSION} (ABI {WIRT_ABI})"
    try:
        result = subprocess.run(
            [*WIRT_COMMAND, "--version"],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        result = None
    if result is None or result.returncode != 0 or result.stdout.strip() != expected:
        print(
            "Install the reviewed Wirt CLI with: "
            "cargo install --locked --git "
            "https://codeberg.org/0xdev/wirt.git --rev "
            f"{WIRT_REV} wirt-cli",
            file=sys.stderr,
        )
        return False
    return True


def validate_package(package: Path) -> bool:
    """Validate one distribution archive with the canonical Wirt command."""
    return _run_wirt("validate", package).returncode == 0


def _package_path(plugin_dir: Path) -> Path:
    manifest_path = plugin_dir / "plugin.toml"
    with manifest_path.open("rb") as handle:
        manifest = tomllib.load(handle)
    plugin = manifest.get("plugin", {})
    plugin_id = plugin.get("id")
    version = plugin.get("version")
    if not isinstance(plugin_id, str) or not isinstance(version, str):
        raise ValueError("plugin manifest is missing its id or version")
    if any(separator in value for value in (plugin_id, version) for separator in "/\\"):
        raise ValueError("plugin id or version is unsafe for an archive filename")
    return plugin_dir / f"{plugin_id}-{version}.wirt"


def _remove_generated_root_artifacts(plugin_dir: Path) -> None:
    for package in plugin_dir.glob("*.wirt"):
        package.unlink()
    for component in plugin_dir.glob("*.wasm"):
        if component.name not in PRESERVED_ROOT_WASM:
            component.unlink()


def _is_preserved_root_component(artifact: Path) -> bool:
    return (
        artifact.name in PRESERVED_ROOT_WASM
        and artifact.parent == PLUGINS_DIR / "facade-test-fixture"
    )


def build() -> int:
    """Build validated Wirt archives. Return 1 if any project fails."""
    print("Building Wirt plugins...")
    print(f"  Target: {WASM_TARGET}")
    if not ensure_target():
        return 1
    if not verify_wirt_cli():
        return 1

    failures: list[str] = []
    for plugin_dir in sorted(PLUGINS_DIR.iterdir()):
        cargo_toml = plugin_dir / "Cargo.toml"
        if not plugin_dir.is_dir() or not cargo_toml.exists():
            continue

        name = plugin_dir.name
        print(f"\n  Building {name}...")
        _remove_generated_root_artifacts(plugin_dir)
        try:
            package = _package_path(plugin_dir)
        except (OSError, tomllib.TOMLDecodeError, ValueError) as error:
            print(f"  ERROR: Invalid manifest for {name}: {error}")
            failures.append(name)
            continue

        result = _run_wirt("build", plugin_dir)
        if result.returncode != 0:
            print(f"  ERROR: Failed to build {name}")
            failures.append(name)
            continue

        result = _run_wirt("package", plugin_dir, "--output", package)
        if result.returncode != 0 or not package.is_file() or package.is_symlink():
            package.unlink(missing_ok=True)
            print(f"  ERROR: Wirt package was not created for {name}")
            failures.append(name)
            continue

        if not validate_package(package):
            package.unlink(missing_ok=True)
            print(f"  ERROR: Wirt package failed validation for {name}")
            failures.append(name)
            continue

        print(f"  Built: {package.name} ({package.stat().st_size:,} bytes)")

    if failures:
        print(f"\nWARNING: failed plugins: {', '.join(failures)}")
        return 1
    return 0


def clean() -> int:
    """Remove generated archives/components and run Cargo clean per project."""
    print("Cleaning Wirt plugins...")
    removed = 0
    for artifact in [*PLUGINS_DIR.rglob("*.wirt"), *PLUGINS_DIR.rglob("*.wasm")]:
        if _is_preserved_root_component(artifact):
            continue
        artifact.unlink()
        removed += 1
    print(f"  Removed {removed} generated plugin artifact(s)")

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
