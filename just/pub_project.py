"""Read Pub packages/workspaces with real YAML semantics and SDK-owned membership."""
from __future__ import annotations

import json
import os
from pathlib import Path
import re
import subprocess
from typing import Any

from release_core import project_path
import release_policy as policy
import pub_sdk


def roots(root: Path, manifests: list[str] | None = None) -> list[str]:
    candidates = list(manifests or [])
    if project_path(root, "pubspec.yaml", exists=False).is_file():
        candidates.insert(0, "pubspec.yaml")
    for settings in policy.config(root).get("monorepo", {}).get("packages", {}).values():
        relative = (Path(settings["path"]) / "pubspec.yaml").as_posix()
        if project_path(root, relative, exists=False).is_file():
            candidates.append(relative)
    return list(dict.fromkeys(candidates))


def check_overrides(root: Path, directory: Path) -> None:
    while directory.is_relative_to(root):
        if (directory / "pubspec_overrides.yaml").exists() or (directory / "pubspec_overrides.yaml").is_symlink():
            raise ValueError(f"Release tooling cannot use pubspec_overrides.yaml: {directory}")
        if directory == root:
            break
        directory = directory.parent


def read(executable: Path, files: list[Path]) -> dict[str, dict[str, Any]]:
    result = pub_sdk.reader_request(executable, {"command": "read", "files": [str(p) for p in files]})
    return {str(p): result[str(p)] for p in files}


def check_workspace(root: Path, path: Path, data: dict[str, Any]) -> None:
    """Check lexical member paths before Pub canonicalizes symlink targets."""
    members = data.get("workspace", [])
    if not isinstance(members, list) or any(not isinstance(m, str) for m in members):
        raise ValueError(f"Workspace paths must be a list: {path}")
    for member in members:
        pattern = ((path.parent / member) / "pubspec.yaml").relative_to(root).as_posix()
        project_path(root, pattern, exists=False)
        for match in path.parent.glob(member):
            project_path(root, (match / "pubspec.yaml").relative_to(root).as_posix())


def discover(root: Path, sdk: dict[str, str], manifests: list[str] | None = None) -> dict[str, dict[str, Any]]:
    executable = pub_sdk.validate(root, sdk)
    candidates = roots(root, manifests)
    if not candidates:
        raise ValueError("No Pub manifest found; supply --manifest path/pubspec.yaml")
    packages: dict[str, dict[str, Any]] = {}
    names: set[str] = set()
    visited: set[str] = set()
    for relative in candidates:
        path = project_path(root, relative)
        relative = path.relative_to(root).as_posix()
        if relative in visited:
            continue
        check_overrides(root, path.parent)
        initial = read(executable, [path])[str(path)]
        check_workspace(root, path, initial)
        # Pub owns workspace globs/nesting. No dependency solve/get is needed.
        result = subprocess.run([str(executable), "pub", "workspace", "list", "--json"], cwd=path.parent,
                                env=dict(os.environ, PUB_HOSTED_URL="https://pub.dev"),
                                capture_output=True, text=True, check=True)
        members = json.loads(result.stdout)["packages"]
        paths: list[Path] = []
        for member in members:
            directory = Path(member["path"]).absolute()
            if not directory.is_relative_to(root):
                raise ValueError("Pub workspace member escapes the checkout")
            manifest = (directory / "pubspec.yaml").relative_to(root).as_posix()
            candidate = project_path(root, manifest)
            check_overrides(root, candidate.parent)
            paths.append(candidate)
        # workspace list emits the actual root first, including private roots.
        if not paths:
            raise ValueError("Pub returned no workspace packages")
        workspace = paths[0].relative_to(root).as_posix()
        values = read(executable, paths)
        for manifest_path in paths:
            key = manifest_path.relative_to(root).as_posix()
            if key in visited:
                continue
            data = values[str(manifest_path)]
            check_workspace(root, manifest_path, data)
            name = data.get("name")
            if not isinstance(name, str) or not re.fullmatch(r"[a-z_][a-z0-9_]*", name):
                raise ValueError(f"Invalid Dart package name in {key}")
            if name in names:
                raise ValueError(f"Duplicate Dart package name: {name}")
            names.add(name)
            dependencies = data.get("dependencies") or {}
            dev = data.get("dev_dependencies") or {}
            if not isinstance(dependencies, dict) or not isinstance(dev, dict):
                raise ValueError(f"Pub dependencies must be mappings: {key}")
            needs_flutter = any(isinstance(v, dict) and v.get("sdk") == "flutter"
                                for v in [*dependencies.values(), *dev.values()]) or "flutter" in data
            packages[key] = dict(data, manifest=key, workspace=workspace, **{"requires-flutter": needs_flutter})
            visited.add(key)
    return packages


def eligible(package: dict[str, Any]) -> bool:
    return package.get("publish_to") in (None, "https://pub.dev", "https://pub.dev/")


def validate(root: Path, settings: dict[str, Any]) -> dict[str, dict[str, Any]]:
    sdk = settings.get("sdk")
    if not isinstance(sdk, dict):
        raise ValueError("pub.dev sdk needs kind dart/flutter and exact version")
    pub_sdk.validate(root, sdk)
    entries = settings.get("packages")
    if not isinstance(entries, list):
        raise ValueError("pub.dev packages must be a list")
    data = discover(root, sdk, [e["manifest"] for e in entries])
    policy.validate_entries(root, entries, data)
    for entry in entries:
        package = data[entry["manifest"]]
        if not eligible(package):
            raise ValueError(f"Package excludes pub.dev: {entry['manifest']}")
        if package["requires-flutter"] and sdk["kind"] != "flutter":
            raise ValueError("Flutter packages need the pinned Flutter SDK")
    return data
