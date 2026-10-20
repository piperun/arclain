"""Explicit crates.io publishing, separate from Git-host release attachments.

Cargo owns packaging, verification, credentials and dependency order. This
command requires existing Cog tags and never bumps versions, tags or pushes.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
from typing import Any

# Release validation must not create untracked bytecode in consumer checkouts.
if __name__ == "__main__":
    sys.dont_write_bytecode = True

from release_core import ROOT, project_path
import rust_project as rust


def selection(root: Path, profile: dict[str, Any], selectors: list[str], all_packages: bool = False,
              interactive: bool = False) -> tuple[dict[str, dict[str, Any]], list[str]]:
    packages = rust.validate_profile(root, profile)
    eligible = [p for unit in profile["releases"] for p in unit["registry-packages"]]
    if not eligible:
        raise ValueError("No crates.io packages are configured; publish=false crates remain excluded")
    if all_packages and selectors:
        raise ValueError("Choose --all or --package, not both")
    selected: list[str] = []
    if all_packages:
        selected = eligible
    elif selectors:
        for selector in selectors:
            matches = [p for p in eligible if selector in (p, packages[p]["name"])]
            if len(matches) != 1:
                raise ValueError(f"Unknown or ambiguous configured registry package: {selector}")
            if matches[0] in selected:
                raise ValueError("Duplicate registry package selection")
            selected.append(matches[0])
    elif len(eligible) == 1:
        selected = eligible
    elif interactive:
        for index, manifest in enumerate(eligible, 1):
            package = packages[manifest]
            print(f"  {index}) {package['name']} {package['version']} ({manifest})")
        answer = input("Crates to publish (numbers separated by commas, or all): ").strip()
        if answer == "all":
            selected = eligible
        else:
            try:
                indices = [int(item.strip()) - 1 for item in answer.split(",")]
                if len(set(indices)) != len(indices) or any(i < 0 or i >= len(eligible) for i in indices):
                    raise ValueError
                selected = [eligible[i] for i in indices]
            except ValueError as error:
                raise ValueError("Choose listed crate numbers or all") from error
    else:
        raise ValueError("Select crates with --package NAME (repeatable) or --all")
    return packages, selected


def publish(root: Path, profile: dict[str, Any], selectors: list[str], *, dry_run: bool = False,
            all_packages: bool = False, interactive: bool = False) -> None:
    packages, selected = selection(root, profile, selectors, all_packages, interactive)
    rust.committed_locks(root, packages, selected)
    # Check every unit/tag and cross-workspace dependency before any Cargo upload.
    for unit in profile["releases"]:
        if set(selected) & set(unit["registry-packages"]):
            tag = rust.tag_for(root, unit, rust.version_at(root, unit["version-manifest"]))
            rust.context(root, profile, unit["id"], tag)
    selected_paths = {str((root / p).parent.resolve()): p for p in selected}
    for manifest in selected:
        package = packages[manifest]
        for dependency in package.get("dependencies", []):
            path = dependency.get("path")
            prerequisite = selected_paths.get(str(Path(path).resolve())) if path else None
            if prerequisite and packages[prerequisite]["workspace"] != package["workspace"]:
                raise ValueError("Selected path dependencies span Cargo workspaces; publish the prerequisite "
                                 "workspace first, then explicitly select the remaining crates")
    groups: dict[str, list[str]] = {}
    for manifest in selected:
        package = packages[manifest]
        groups.setdefault(package["workspace"], []).append(manifest)
        print(f"crates-io: {package['name']} {package['version']}", flush=True)
    help_text = rust.cargo(root, profile["rust-toolchain"], "publish", "--help",
                           capture_output=True, text=True).stdout
    if "--workspace" not in help_text:
        raise ValueError("This Cargo lacks stable multi-package publishing; select a newer installed toolchain")
    commands: list[list[str]] = []
    for workspace, members in groups.items():
        command = ["publish", "--locked", "--registry", "crates-io", "--manifest-path", str(root / workspace)]
        for manifest in members:
            package = packages[manifest]
            command.extend(["--package", f"{package['name']}@{package['version']}"])
        commands.append(command)
    for command in commands:
        rust.cargo(root, profile["rust-toolchain"], *command, "--dry-run")
    if dry_run:
        print("Cargo dry run complete; no crates uploaded.")
        return
    for command in commands:
        try:
            rust.cargo(root, profile["rust-toolchain"], *command)
        except subprocess.CalledProcessError as error:
            names = ", ".join(packages[p]["name"] for p in selected)
            raise ValueError(f"Cargo publishing stopped for selection [{names}] (exit {error.returncode}). "
                             "Some crates may already be uploaded or awaiting indexing. Check crates.io "
                             "and explicitly select only remaining crates; no automatic retry was made.") from error
    print("Cargo completed crates.io publishing for the selected crates.")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", default="packaging/release.json")
    parser.add_argument("--package", action="append", default=[])
    parser.add_argument("--all", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--non-interactive", action="store_true")
    args = parser.parse_args(argv)
    try:
        profile = json.loads(project_path(ROOT, args.profile).read_text())
        ci = os.environ.get("CI", "").lower() not in ("", "0", "false", "no")
        interactive = sys.stdin.isatty() and sys.stdout.isatty() and not ci and not args.non_interactive
        publish(ROOT, profile, args.package, dry_run=args.dry_run, all_packages=args.all, interactive=interactive)
        return 0
    except (EOFError, KeyboardInterrupt):
        print("Publishing cancelled.", file=sys.stderr)
        return 130
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        print(f"Registry publishing failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
