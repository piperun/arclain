#!/usr/bin/env python3
"""Format only Cargo packages and manifests owned by this repository."""
from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
ROOT_PACKAGES = (
    "arclain_app",
    "arclain_app_fs",
    "arclain_checksum",
    "arclain_core",
    "arclain_data",
    "arclain_db",
    "arclain-network",
    "arclain_plugins",
    "arclain_signals",
    "arclain_theme",
    "arclain_ui",
    "arclain_widgets",
)
STANDALONE_MANIFESTS = (
    "plugin-sdk/Cargo.toml",
    "plugins/dlsite-metadata/Cargo.toml",
    "plugins/facade-test-fixture/Cargo.toml",
    "plugins/gstreamer-preview/Cargo.toml",
    "plugins/ui-demo/Cargo.toml",
)


def commands(*, check: bool) -> list[list[str]]:
    """Build rustfmt commands constrained to Arclain-owned crates."""
    root_command = ["cargo", "fmt"]
    for package in ROOT_PACKAGES:
        root_command.extend(("--package", package))

    result = [root_command]
    result.extend(
        ["cargo", "fmt", "--manifest-path", manifest]
        for manifest in STANDALONE_MANIFESTS
    )
    if check:
        for command in result:
            command.extend(("--", "--check"))
    return result


def format_owned(*, check: bool) -> int:
    """Run the scoped formatter and return its first nonzero status."""
    for command in commands(check=check):
        result = subprocess.run(command, cwd=REPO_ROOT)
        if result.returncode != 0:
            return result.returncode
    return 0


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="report formatting differences without writing files",
    )
    args = parser.parse_args()
    sys.exit(format_owned(check=args.check))


if __name__ == "__main__":
    main()
