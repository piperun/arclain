#!/usr/bin/env python3
"""Cargo dependency inspector / updater.

Usage:
    python scripts/_deps.py                          # outdated check
    python scripts/_deps.py --update [--dry-run]     # bump Cargo.lock
    python scripts/_deps.py --upgrade [--incompatible] [--dry-run]
                                                     # bump Cargo.toml (needs cargo-edit)

`--update`   runs `cargo update` (lockfile only).
`--upgrade`  runs `cargo upgrade --workspace` (manifests too).
With no flag, runs `cargo outdated` if installed, else `cargo update --dry-run`.
"""
from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent


def have_command(name: str) -> bool:
    return shutil.which(name) is not None


def main() -> None:
    parser = argparse.ArgumentParser(description="Cargo dependency tools.")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--update", action="store_true",
        help="Run `cargo update` (Cargo.lock)",
    )
    mode.add_argument(
        "--upgrade", action="store_true",
        help="Run `cargo upgrade --workspace` (needs cargo-edit)",
    )
    parser.add_argument(
        "--incompatible", action="store_true",
        help="With --upgrade, also bump to incompatible/breaking versions",
    )
    parser.add_argument(
        "--dry-run", action="store_true",
        help="Show what would change without writing",
    )
    args = parser.parse_args()

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
        sys.exit(subprocess.run(cmd, cwd=REPO_ROOT).returncode)

    if args.update:
        cmd = ["cargo", "update"]
        if args.dry_run:
            cmd.append("--dry-run")
            print("(Dry run - no changes will be made)")
        sys.exit(subprocess.run(cmd, cwd=REPO_ROOT).returncode)

    # Default: outdated check
    if have_command("cargo-outdated"):
        print("Using cargo-outdated:")
        sys.exit(subprocess.run(
            ["cargo", "outdated", "--workspace"], cwd=REPO_ROOT,
        ).returncode)

    print("Note: install 'cargo-outdated' for a detailed report:")
    print("  cargo install cargo-outdated\n")
    print("Falling back to 'cargo update --dry-run':")
    sys.exit(subprocess.run(
        ["cargo", "update", "--dry-run"], cwd=REPO_ROOT,
    ).returncode)


if __name__ == "__main__":
    main()
