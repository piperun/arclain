"""Known registry configuration adapters, including legacy native Rust profiles."""
from __future__ import annotations

from pathlib import Path
from typing import Any

import release_policy as policy

REGISTRIES = ("crates-io", "pub.dev")


def configured(profile: dict[str, Any]) -> list[str]:
    if not isinstance(profile, dict):
        raise ValueError("Release profile must be an object")
    table = profile.get("registries", {})
    if not isinstance(table, dict) or any(key not in REGISTRIES for key in table):
        raise ValueError("Supported registries are crates-io and pub.dev")
    if any(not isinstance(value, dict) for value in table.values()):
        raise ValueError("Registry settings must be objects")
    names = list(table)
    if profile.get("kind") == "rust" and "crates-io" not in names:
        if any(unit.get("registry-packages") for unit in profile.get("releases", [])):
            names.insert(0, "crates-io")
    return names


def sdk(settings: dict[str, Any], kinds: tuple[str, ...]) -> dict[str, str]:
    value = settings.get("sdk")
    if (not isinstance(value, dict) or set(value) != {"kind", "version"}
            or value["kind"] not in kinds or not isinstance(value["version"], str)):
        raise ValueError("Registry sdk needs a supported kind and exact version")
    return value


def cargo_profile(root: Path, profile: dict[str, Any]) -> dict[str, Any]:
    import rust_project as rust
    configured(profile)
    settings = profile.get("registries", {}).get("crates-io")
    if settings is None:
        if profile.get("kind") != "rust":
            raise ValueError("No crates.io packages configured")
        return profile
    pin = sdk(settings, ("rust",))["version"]
    entries = settings.get("packages")
    if not isinstance(entries, list):
        raise ValueError("crates-io packages must be a list")
    metadata = rust.discover(root, pin, [e["manifest"] for e in entries])
    policy.validate_entries(root, entries, metadata)
    if any(not rust.registry_eligible(metadata[e["manifest"]]) for e in entries):
        raise ValueError("Registry selection includes a crate that excludes crates-io")
    if profile.get("kind") == "rust":
        legacy = {p: u["cog-package"] for u in profile["releases"] for p in u["registry-packages"]}
        if any(e["manifest"] in legacy and legacy[e["manifest"]] != e["cog-package"] for e in entries):
            raise ValueError("Explicit and native Cargo Cog mappings disagree")
    groups: dict[str | None, list[str]] = {}
    for entry in entries:
        groups.setdefault(entry["cog-package"], []).append(entry["manifest"])
    units = [{"id": alias or "root", "name": alias or metadata[members[0]]["name"],
              "cog-package": alias, "version-manifest": members[0],
              "packages": [{"manifest": m} for m in members], "registry-packages": members}
             for alias, members in groups.items()]
    return {"kind": "rust", "registry-only": True, "rust-toolchain": pin, "releases": units}


def select_packages(entries: list[dict[str, Any]], metadata: dict[str, dict[str, Any]], selectors: list[str],
                    all_packages: bool, interactive: bool) -> list[dict[str, Any]]:
    if not entries:
        raise ValueError("No eligible registry packages configured")
    if all_packages and selectors:
        raise ValueError("Choose --all or --package, not both")
    if all_packages:
        return entries
    if not selectors:
        if len(entries) == 1:
            return entries
        if not interactive:
            raise ValueError("Select packages with --package NAME (repeatable) or --all")
        for index, entry in enumerate(entries, 1):
            p = metadata[entry["manifest"]]
            print(f"  {index}) {p['name']} {p['version']} ({entry['manifest']})")
        answer = input("Packages (comma-separated numbers, or all): ").strip()
        if answer == "all":
            return entries
        try:
            indices = [int(v.strip()) - 1 for v in answer.split(",")]
            if len(set(indices)) != len(indices) or any(i < 0 or i >= len(entries) for i in indices):
                raise ValueError
            return [entries[i] for i in indices]
        except ValueError as error:
            raise ValueError("Choose listed package numbers or all") from error
    selected = []
    for selector in selectors:
        matches = [e for e in entries if selector in (e["manifest"], metadata[e["manifest"]]["name"])]
        if len(matches) != 1 or matches[0] in selected:
            raise ValueError(f"Unknown, duplicate or ambiguous registry package: {selector}")
        selected.append(matches[0])
    return selected


def metadata_for(root: Path, profile: dict[str, Any], name: str) -> tuple[list[dict[str, Any]], dict[str, dict[str, Any]]]:
    if name == "pub.dev":
        import pub_project
        settings = profile["registries"][name]
        return settings["packages"], pub_project.validate(root, settings)
    import rust_project as rust
    normalized = cargo_profile(root, profile)
    data = rust.validate_profile(root, normalized)
    entries = [{"manifest": p, "cog-package": u["cog-package"]}
               for u in normalized["releases"] for p in u["registry-packages"]]
    return entries, data


def check_tag(root: Path, profile: dict[str, Any], tag: str) -> tuple[str, str]:
    if not tag:
        raise ValueError("Supply the Cog release tag or set RELEASE_TAG")
    selected: list[tuple[list[dict[str, Any]], dict[str, dict[str, Any]]]] = []
    for name in configured(profile):
        entries, data = metadata_for(root, profile, name)
        matching = [e for e in entries if policy.cog_tag(root, e["cog-package"], data[e["manifest"]]["version"]) == tag]
        if matching:
            selected.append((matching, data))
    if not selected:
        raise ValueError(f"No configured registry release matches {tag!r}")
    commits = [sha for entries, data in selected for sha in policy.validate_tags(root, entries, data).values()]
    return tag, commits[0]
