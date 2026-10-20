"""Explicit native Pub preflights and uploads, without bypasses or automatic retries."""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import urlopen

from release_core import git
import release_policy as policy
import registry
import pub_project
import pub_sdk


def published_versions(name: str) -> list[str]:
    url = f"https://pub.dev/api/packages/{quote(name, safe='')}"
    try:
        with urlopen(url, timeout=20) as response:
            # A pathological registry response must not cause unbounded reads.
            body = response.read(4 * 1024 * 1024 + 1)
        if len(body) > 4 * 1024 * 1024:
            raise ValueError("pub.dev dependency metadata exceeds 4 MiB")
        data = json.loads(body)
        return [v["version"] for v in data["versions"] if not v.get("retracted", False)]
    except HTTPError as error:
        error.close()
        if error.code == 404:
            return []
        raise ValueError(f"Cannot establish published dependency versions for {name}: HTTP {error.code}") from error
    except (URLError, TimeoutError, OSError) as error:
        raise ValueError(f"Cannot establish published dependency versions for {name}: {error}") from error


def check_dependencies(root: Path, sdk: dict[str, str], selected: list[dict[str, Any]],
                       metadata: dict[str, dict[str, Any]]) -> None:
    executable = pub_sdk.validate(root, sdk)
    by_name = {p["name"]: p for p in metadata.values()}
    overridden = {p["workspace"] for p in metadata.values() if p.get("dependency_overrides")}
    versions: dict[str, list[str]] = {}
    for entry in selected:
        package = metadata[entry["manifest"]]
        if package["workspace"] in overridden:
            raise ValueError("Remove dependency_overrides before registry publication; they can mask unavailable dependencies")
        for name, dependency in (package.get("dependencies") or {}).items():
            other = by_name.get(name)
            if not other or other["workspace"] != package["workspace"]:
                continue
            if dependency is None or isinstance(dependency, str):
                constraint = dependency if dependency is not None else "any"
            elif isinstance(dependency, dict) and not any(k in dependency for k in ("path", "git", "sdk")):
                hosted = dependency.get("hosted", "https://pub.dev")
                if isinstance(hosted, dict):
                    hosted = hosted.get("url", "https://pub.dev")
                if hosted not in ("https://pub.dev", "https://pub.dev/"):
                    raise ValueError(f"Workspace dependency {name} uses an unsupported custom hosted registry")
                constraint = dependency.get("version", "any")
            else:
                continue  # Native Pub reports invalid production path/Git deps.
            if name not in versions:
                versions[name] = published_versions(name)
            allowed = pub_sdk.reader_request(executable, {"command": "allows", "constraint": constraint,
                                                          "versions": versions[name]})
            if not allowed:
                raise ValueError(f"{package['name']} needs {name} {constraint} on pub.dev. Publish the prerequisite "
                                 "first, wait for availability, then explicitly select the remaining packages.")


def publish(root: Path, settings: dict[str, Any], selectors: list[str], *, dry_run: bool = False,
            all_packages: bool = False, interactive: bool = False) -> None:
    metadata = pub_project.validate(root, settings)
    selected = registry.select_packages(settings["packages"], metadata, selectors, all_packages, interactive)
    sdk = settings["sdk"]
    policy.validate_tags(root, selected, metadata)
    for entry in selected:
        package = metadata[entry["manifest"]]
        print(f"pub.dev: {package['name']} {package['version']}", flush=True)
    check_dependencies(root, sdk, selected, metadata)
    env = dict(os.environ, PUB_HOSTED_URL="https://pub.dev")
    initial = git(root, "rev-parse", "HEAD")
    for entry in selected:
        subprocess.run(pub_sdk.native_command(root, sdk, "publish", "--dry-run"),
                       cwd=(root / entry["manifest"]).parent, env=env, check=True)
    # Re-read versions/eligibility and verify tracked locks/source after Pub get.
    current = pub_project.validate(root, settings)
    policy.validate_tags(root, selected, current)
    if initial != git(root, "rev-parse", "HEAD") or any(metadata[e["manifest"]] != current[e["manifest"]] for e in selected):
        raise ValueError("Source changed during Pub preflight; review changes and retag through Cog")
    if dry_run:
        print("Pub dry run complete; no packages uploaded.")
        return
    completed: list[str] = []
    for index, entry in enumerate(selected):
        package = current[entry["manifest"]]
        # An earlier upload must never hide a source change for the next one.
        for p in current.values():
            pub_project.check_overrides(root, (root / p["manifest"]).parent)
        policy.validate_tags(root, selected, current)
        if initial != git(root, "rev-parse", "HEAD"):
            raise ValueError(f"Source changed before uploading {package['name']}; completed: {completed}")
        try:
            subprocess.run(pub_sdk.native_command(root, sdk, "publish", "--force"),
                           cwd=(root / entry["manifest"]).parent, env=env, check=True)
        except subprocess.CalledProcessError as error:
            remaining = [current[e["manifest"]]["name"] for e in selected[index + 1:]]
            raise ValueError(f"Pub upload failed for {package['name']} (exit {error.returncode}). "
                             f"Completed: {completed}; not attempted: {remaining}. The failed request may "
                             "have reached pub.dev; check the server and explicitly select remaining packages. "
                             "No automatic retry was made.") from error
        completed.append(package["name"])
    print("Pub completed publication for: " + ", ".join(completed))
