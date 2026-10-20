"""Cog release identity shared by native builds and package registries.

Manifests retain their language-specific version rules. Cog aliases own paths,
not package names; selected members of each alias share one release version.
"""
from __future__ import annotations

from pathlib import Path
import re
import subprocess
import tomllib
from typing import Any

from release_core import clean_tag, project_path, safe_name


def config(root: Path) -> dict[str, Any]:
    data = tomllib.loads(project_path(root, "cog.toml").read_text(encoding="utf-8"))
    for value in (data.get("tag_prefix", ""), data.get("monorepo_version_separator", "-")):
        if not isinstance(value, str) or any(c in value for c in "*?[\\\n\r"):
            raise ValueError("Cog tag prefix/separator must be literal Git tag text")
    monorepo = data.get("monorepo", {})
    if not isinstance(monorepo, dict) or not isinstance(monorepo.get("packages", {}), dict):
        raise ValueError("cog.toml monorepo.packages must be a table")
    for alias, settings in monorepo.get("packages", {}).items():
        safe_name(alias)
        if not isinstance(settings, dict) or not isinstance(settings.get("path"), str):
            raise ValueError(f"Cog package {alias} needs a path")
        project_path(root, settings["path"].rstrip("/") + "/placeholder", exists=False)
    return data


def cog_tag(root: Path, alias: str | None, version: str) -> str:
    data = config(root)
    if alias is not None and alias not in data.get("monorepo", {}).get("packages", {}):
        raise ValueError(f"Unknown Cog package: {alias}")
    if not isinstance(version, str) or not re.fullmatch(r"\d+\.\d+\.\d+(?:-[\w.-]+)?(?:\+[\w.-]+)?", version):
        raise ValueError(f"Expected an exact package version: {version!r}")
    prefix = ((alias + data.get("monorepo_version_separator", "-")) if alias else "")
    tag = prefix + data.get("tag_prefix", "") + version
    if subprocess.run(["git", "check-ref-format", f"refs/tags/{tag}"], capture_output=True).returncode:
        raise ValueError(f"Invalid Cog release tag: {tag}")
    return tag


def owners(root: Path, package: dict[str, Any]) -> list[str | None]:
    aliases = []
    for alias, settings in config(root).get("monorepo", {}).get("packages", {}).items():
        path = (root / settings["path"]).resolve()
        if path in ((root / package["manifest"]).parent.resolve(), (root / package["workspace"]).parent.resolve()):
            aliases.append(alias)
    return aliases or [None]


def validate_entries(root: Path, entries: list[dict[str, Any]], metadata: dict[str, dict[str, Any]]) -> None:
    if not isinstance(entries, list) or not entries:
        raise ValueError("Configure at least one registry package")
    seen: set[str] = set()
    versions: dict[str | None, str] = {}
    for entry in entries:
        if not isinstance(entry, dict) or set(entry) != {"manifest", "cog-package"}:
            raise ValueError("Registry packages need manifest and cog-package fields")
        manifest, alias = entry["manifest"], entry["cog-package"]
        project_path(root, manifest)
        if alias is not None and not isinstance(alias, str):
            raise ValueError("cog-package must be an alias or null")
        if manifest in seen:
            raise ValueError(f"Duplicate registry manifest: {manifest}")
        seen.add(manifest)
        if manifest not in metadata:
            raise ValueError(f"Not a discovered package: {manifest}")
        package = metadata[manifest]
        possible = owners(root, package)
        if len(possible) != 1:
            raise ValueError(f"Ambiguous Cog ownership for {manifest}; give each package one owning Cog path")
        if alias not in possible:
            raise ValueError(f"Cog alias {alias!r} does not own {manifest}")
        version = package["version"]
        cog_tag(root, alias, version)
        if alias in versions and versions[alias] != version:
            raise ValueError("Selected packages sharing a Cog alias need the same version; declare independent Cog packages")
        versions[alias] = version


def validate_tags(root: Path, entries: list[dict[str, Any]], metadata: dict[str, dict[str, Any]]) -> dict[str, str]:
    validate_entries(root, entries, metadata)
    workspaces = {metadata[e["manifest"]]["workspace"] for e in entries}
    manifests = [p["manifest"] for p in metadata.values() if p["workspace"] in workspaces]
    for manifest in dict.fromkeys(["cog.toml", *workspaces, *manifests]):
        if subprocess.run(["git", "ls-files", "--error-unmatch", "--", manifest], cwd=root,
                          capture_output=True).returncode:
            raise ValueError(f"Release input must be tracked in Git: {manifest}")
    tags = {e["manifest"]: cog_tag(root, e["cog-package"], metadata[e["manifest"]]["version"]) for e in entries}
    commits = {tag: clean_tag(root, tag) for tag in dict.fromkeys(tags.values())}
    return {manifest: commits[tag] for manifest, tag in tags.items()}
