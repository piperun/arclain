"""Read Cargo workspace membership and Cog release policy without resolving dependencies.

Discovery is limited to declared manifests. It never builds, installs toolchains,
updates locks or guesses independent versions from directory layout.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import re
import subprocess
import tomllib
from typing import Any

import release_policy as policy

from release_core import (ReleaseContext, add_destination, clean_tag, project_path, relative_path,
                          requested_tag, safe_name)

PLATFORMS = {"linux-x86_64": "ubuntu-24.04", "windows-x86_64": "windows-2022"}


def toml(path: Path) -> dict[str, Any]:
    return tomllib.loads(path.read_text(encoding="utf-8"))


def toolchain(root: Path, explicit: str | None = None) -> str:
    channel = explicit
    if not channel:
        for name in ("rust-toolchain.toml", "rust-toolchain"):
            path = project_path(root, name, exists=False)
            if path.is_file():
                channel = (toml(path)["toolchain"]["channel"] if name.endswith(".toml")
                           else path.read_text().strip())
                break
    if channel and re.fullmatch(r"(?:\d+\.\d+\.\d+|nightly-\d{4}-\d\d-\d\d)", channel):
        return channel
    env = dict(os.environ, RUSTUP_AUTO_INSTALL="0")
    if channel:
        env["RUSTUP_TOOLCHAIN"] = channel
    result = subprocess.run(["rustc", "--version"], cwd=root, env=env, check=True,
                            capture_output=True, text=True)
    match = re.match(r"rustc (\d+\.\d+\.\d+) ", result.stdout)
    if not match:
        raise ValueError("Supply --rust-toolchain with an installed exact release or dated nightly")
    return match[1]


def cargo(root: Path, pin: str, *args: str, **kwargs: Any) -> subprocess.CompletedProcess[Any]:
    """Use an installed matching toolchain; Rustup must never auto-install here."""
    env = dict(os.environ, RUSTUP_AUTO_INSTALL="0", RUSTUP_TOOLCHAIN=pin)
    probe = subprocess.run(["rustc", "--version"], env=env, cwd=root, capture_output=True, text=True)
    if probe.returncode and re.fullmatch(r"\d+\.\d+\.\d+", pin):
        installed = subprocess.run(["rustup", "toolchain", "list"], capture_output=True,
                                   text=True, check=True).stdout
        for line in installed.splitlines():
            candidate = line.split()[0]
            env["RUSTUP_TOOLCHAIN"] = candidate
            probe = subprocess.run(["rustc", "--version"], env=env, cwd=root,
                                   capture_output=True, text=True)
            if probe.returncode == 0 and probe.stdout.startswith(f"rustc {pin} "):
                break
        else:
            raise ValueError(f"Install Rust {pin} before running this command")
    elif probe.returncode:
        raise ValueError(f"Install Rust {pin} before running this command")
    return subprocess.run(["cargo", *args], env=env, cwd=root, check=True, **kwargs)


# Compatibility aliases for existing native consumers.
cog_config = policy.config


def tag_for(root: Path, unit: dict[str, Any], version: str) -> str:
    return policy.cog_tag(root, unit["cog-package"], version)


def discover(root: Path, pin: str, manifests: list[str] | None = None) -> dict[str, dict[str, Any]]:
    candidates = list(manifests or [])
    if (root / "Cargo.toml").is_file():
        candidates.insert(0, "Cargo.toml")
    config = cog_config(root)
    for settings in config.get("monorepo", {}).get("packages", {}).values():
        relative = settings["path"].rstrip("/") + "/Cargo.toml"
        if project_path(root, relative, exists=False).is_file():
            candidates.append(relative)
    if not candidates:
        raise ValueError("No Cargo manifest found; supply --manifest path/Cargo.toml")
    packages: dict[str, dict[str, Any]] = {}
    visited: set[str] = set()
    for relative in dict.fromkeys(candidates):
        manifest = project_path(root, relative)
        if relative in visited:
            continue
        result = cargo(root, pin, "metadata", "--format-version", "1", "--no-deps", "--offline", "--locked",
                       "--manifest-path", str(manifest), capture_output=True, text=True)
        data = json.loads(result.stdout)
        workspace = Path(data["workspace_root"]).resolve()
        if not workspace.is_relative_to(root.resolve()):
            raise ValueError("Cargo workspace root escapes the checkout")
        lock = workspace / "Cargo.lock"
        project_path(root, lock.relative_to(root).as_posix())
        workspace_manifest = (workspace / "Cargo.toml").relative_to(root).as_posix()
        visited.add(workspace_manifest)
        for package in data["packages"]:
            if package["id"] not in data["workspace_members"]:
                continue
            path = Path(package["manifest_path"]).absolute()
            if not path.is_relative_to(root):
                raise ValueError("Cargo package manifest escapes the checkout")
            key = path.relative_to(root).as_posix()
            project_path(root, key)
            visited.add(key)
            packages[key] = dict(package, workspace=workspace_manifest, manifest=key)
    return packages


def committed_locks(root: Path, packages: dict[str, dict[str, Any]], manifests: list[str]) -> None:
    for workspace in dict.fromkeys(packages[m]["workspace"] for m in manifests):
        lock = (Path(workspace).parent / "Cargo.lock").as_posix()
        result = subprocess.run(["git", "ls-files", "--error-unmatch", "--", lock],
                                cwd=root, capture_output=True)
        if result.returncode:
            raise ValueError(f"Release builds need a committed Cargo.lock: {lock}")


def version_at(root: Path, manifest: str) -> str:
    data = toml(project_path(root, manifest))
    value = data.get("package", {}).get("version")
    if isinstance(value, dict) and value.get("workspace"):
        # Follow Cargo's explicit workspace pointer or find its containing root.
        directory = (root / manifest).parent
        explicit = data["package"].get("workspace")
        if explicit:
            directory = (directory / explicit).resolve()
        while directory.is_relative_to(root):
            candidate = directory / "Cargo.toml"
            if candidate.is_file():
                workspace_version = toml(candidate).get("workspace", {}).get("package", {}).get("version")
                if isinstance(workspace_version, str):
                    value = workspace_version
                    break
            if directory == root:
                break
            directory = directory.parent
    if value is None:
        value = data.get("workspace", {}).get("package", {}).get("version")
    if not isinstance(value, str) or not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", value):
        raise ValueError(f"Missing resolved release version in {manifest}")
    return value


def registry_eligible(package: dict[str, Any]) -> bool:
    registries = package.get("publish")
    return registries is None or "crates-io" in registries


def enabled_features(package: dict[str, Any], features: list[str], defaults: bool) -> set[str]:
    enabled = set(features)
    if defaults:
        enabled.add("default")
    pending = list(enabled)
    while pending:
        for feature in package.get("features", {}).get(pending.pop(), []):
            if feature not in enabled:
                enabled.add(feature)
                pending.append(feature)
    return enabled


def validate_profile(root: Path, profile: dict[str, Any], *, metadata: bool = True) -> dict[str, dict[str, Any]]:
    if profile.get("kind") != "rust":
        raise ValueError("This command requires a Rust release profile")
    pin = profile["rust-toolchain"]
    if not isinstance(pin, str) or not re.fullmatch(r"(?:\d+\.\d+\.\d+|nightly-\d{4}-\d\d-\d\d)", pin):
        raise ValueError("rust-toolchain must be an exact release or dated nightly")
    units = profile["releases"]
    if not isinstance(units, list) or not units:
        raise ValueError("Configure at least one Rust release unit")
    manifests = [p["manifest"] for unit in units for p in unit["packages"]]
    packages = discover(root, pin, manifests) if metadata else {}
    if profile.get("registry-only"):
        entries = [{"manifest": p["manifest"], "cog-package": u["cog-package"]}
                   for u in units for p in u["packages"]]
        if metadata:
            policy.validate_entries(root, entries, packages)
            if any(not registry_eligible(packages[e["manifest"]]) for e in entries):
                raise ValueError("Registry selection includes a crate that excludes crates-io")
        return packages
    ids: set[str] = set()
    names: set[str] = set()
    owned: set[str] = set()
    aliases: set[str | None] = set()
    cog = cog_config(root).get("monorepo", {}).get("packages", {})
    for unit in units:
        identifier, name, alias = unit["id"], unit["name"], unit["cog-package"]
        safe_name(identifier)
        safe_name(name)
        if identifier in ids or name.casefold() in names or alias in aliases:
            raise ValueError("Duplicate release id, name or Cog alias")
        ids.add(identifier)
        names.add(name.casefold())
        aliases.add(alias)
        if alias is not None and alias not in cog:
            raise ValueError(f"Unknown Cog package: {alias}")
        version = version_at(root, unit["version-manifest"])
        tag = tag_for(root, unit, version)
        if subprocess.run(["git", "check-ref-format", f"refs/tags/{tag}"], capture_output=True).returncode:
            raise ValueError(f"Invalid Cog release tag: {tag}")
        anchor = ""
        if alias:
            anchor = (cog[alias]["path"].rstrip("/") + "/Cargo.toml").removeprefix("./")
            if unit["version-manifest"] != anchor:
                raise ValueError(f"Cog alias {alias} must use its configured version manifest")
        if not unit["packages"] or not unit["platforms"] or len(set(unit["platforms"])) != len(unit["platforms"]):
            raise ValueError("Release units need packages and distinct platforms")
        if any(p not in PLATFORMS for p in unit["platforms"]):
            raise ValueError("Supported platforms: " + ", ".join(PLATFORMS))
        binaries: set[str] = set()
        selected: set[str] = set()
        for entry in unit["packages"]:
            manifest = entry["manifest"]
            project_path(root, manifest)
            if manifest in owned:
                raise ValueError(f"Cargo package belongs to multiple releases: {manifest}")
            owned.add(manifest)
            selected.add(manifest)
            if version_at(root, manifest) != version:
                raise ValueError("Selected packages must agree with the release anchor version")
            package = packages.get(manifest)
            if metadata and package is None:
                raise ValueError(f"Not a Cargo workspace member: {manifest}")
            if alias and manifest != anchor:
                if metadata and (not package or package["workspace"] != anchor):
                    raise ValueError(f"Cog alias {alias} does not own {manifest}")
            features = entry["features"]
            if not isinstance(features, list) or any(not isinstance(f, str) or not re.fullmatch(r"[\w/?.+-]+", f) for f in features):
                raise ValueError("features must be a list of Cargo feature names")
            if package and any("/" not in f and f not in package.get("features", {}) for f in features):
                raise ValueError(f"Unknown Cargo feature in {manifest}")
            if not isinstance(entry["default-features"], bool):
                raise ValueError("default-features must be boolean")
            if len(set(entry["binaries"])) != len(entry["binaries"]):
                raise ValueError("Duplicate binary target")
            for binary in entry["binaries"]:
                safe_name(binary)
                if binary.casefold() in binaries:
                    raise ValueError(f"Duplicate binary archive destination: {binary}")
                binaries.add(binary.casefold())
                if package:
                    target = next((t for t in package["targets"] if t["name"] == binary and "bin" in t["kind"]), None)
                    if not target:
                        raise ValueError(f"Unknown binary {binary} in {manifest}")
                    required = set(target.get("required-features", []))
                    enabled = enabled_features(package, features, entry["default-features"])
                    if not required.issubset(enabled):
                        raise ValueError(f"Binary {binary} needs explicit features: {sorted(required - enabled)}")
        registry = unit["registry-packages"]
        if len(set(registry)) != len(registry) or not set(registry).issubset(selected):
            raise ValueError("Registry packages must be distinct selected Cargo manifests")
        if metadata and any(not registry_eligible(packages[p]) for p in registry):
            raise ValueError("Registry selection includes a crate that excludes crates-io")
        for path in unit.get("licenses", []):
            project_path(root, path)
        project_path(root, unit["changelog"], exists=False)
        for asset in unit.get("assets", []):
            project_path(root, asset["path"])
            relative_path(asset["destination"])
            if any(p not in unit["platforms"] for p in asset.get("platforms", unit["platforms"])):
                raise ValueError("Asset platform is not selected by its release unit")
        for platform in unit["platforms"]:
            destinations: set[str] = set()
            for entry in unit["packages"]:
                for binary in entry["binaries"]:
                    add_destination(destinations, binary + (".exe" if platform.startswith("windows") else ""))
            for license_path in unit.get("licenses", []):
                add_destination(destinations, "licenses/" + relative_path(license_path))
            for asset in unit.get("assets", []):
                if platform in asset.get("platforms", unit["platforms"]):
                    add_destination(destinations, asset["destination"])
            if platform.startswith("windows"):
                for dll in ("msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"):
                    add_destination(destinations, dll)

    return packages


def select_unit(root: Path, profile: dict[str, Any], selector: str = "", tag: str = "") -> dict[str, Any]:
    tag = requested_tag(tag)
    units = [u for u in profile["releases"] if not selector or u["id"] == selector]
    if tag:
        units = [u for u in units if tag_for(root, u, version_at(root, u["version-manifest"])) == tag]
    if len(units) != 1:
        raise ValueError("Select exactly one release with --unit, or set its matching RELEASE_TAG")
    return units[0]


def artifact_names(unit: dict[str, Any], version: str) -> tuple[str, ...]:
    stem = f"{unit['name']}-v{version}"
    if not any(p["binaries"] for p in unit["packages"]):
        return (stem + "-source.tar.gz",)
    return tuple(stem + "-" + p + (".zip" if p.startswith("windows") else ".tar.gz") for p in unit["platforms"])


def context(root: Path, profile: dict[str, Any], selector: str = "", tag: str = "") -> ReleaseContext:
    validate_profile(root, profile, metadata=False)
    unit = select_unit(root, profile, selector, tag)
    version = version_at(root, unit["version-manifest"])
    expected = tag_for(root, unit, version)
    actual = requested_tag(tag) or expected
    if actual != expected:
        raise ValueError(f"Release tag must match Cargo/Cog: expected {expected}")
    commit = clean_tag(root, actual)
    if profile.get("registry-only"):
        return ReleaseContext(unit["name"], version, actual, commit, "CHANGELOG.md", ())
    return ReleaseContext(unit["name"], version, actual, commit, unit["changelog"],
                          artifact_names(unit, version), not any(p["binaries"] for p in unit["packages"]))
