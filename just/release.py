#!/usr/bin/env python3
"""Validate, collect and publish application releases to GitHub and Codeberg."""
from __future__ import annotations

import argparse
import http.client
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Any
from urllib.parse import quote, urlencode, urljoin, urlsplit
import uuid

# Release validation must not create untracked bytecode in consumer checkouts.
if __name__ == "__main__":
    sys.dont_write_bytecode = True

from release_core import ROOT, ReleaseContext, digest, git, run
import flutter
import rust_project
import rust_package
import registry
from release_core import requested_tag

# Backward-compatible Flutter adapter names.
app_version = flutter.app_version
artifact_name = flutter.artifact_name
release_tag = flutter.release_tag


def load_profile(root: Path, relative: str) -> dict[str, Any]:
    profile = json.loads((root / relative).read_text(encoding="utf-8"))
    registry.configured(profile)
    if profile.get("kind") == "packages":
        if not registry.configured(profile):
            raise ValueError("No registry outputs configured")
        return profile
    if profile.get("kind", "flutter") == "flutter":
        return flutter.load_profile(root, relative)
    rust_project.validate_profile(root, profile, metadata=False)
    return profile


def context(root: Path, profile: dict[str, Any]) -> ReleaseContext:
    if profile.get("kind") == "rust":
        return rust_project.context(root, profile)
    tag, commit = validate_release(root, profile)
    return ReleaseContext(profile["name"], app_version(root), tag, commit, "CHANGELOG.md", ())


def validate_release(root: Path, profile: dict[str, Any], tag: str = "") -> tuple[str, str]:
    if profile.get("kind") == "packages":
        return registry.check_tag(root, profile, requested_tag(tag))
    if profile.get("kind") == "rust":
        ctx = rust_project.context(root, profile, tag=tag)
        return ctx.tag, ctx.commit
    tag = tag or os.environ.get("RELEASE_TAG") or os.environ.get("GITHUB_REF_NAME", "")
    expected = release_tag(root, profile)
    if tag != expected:
        raise ValueError(f"Release tag must match pubspec.yaml: expected {expected}, got {tag!r}")
    commit = git(root, "rev-parse", "HEAD")
    if git(root, "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}") != commit:
        raise ValueError("Check out the release tag itself before building or publishing")
    if (root / "pubspec_overrides.yaml").exists():
        raise ValueError("Release builds cannot use pubspec_overrides.yaml")
    if git(root, "status", "--porcelain", "--untracked-files=normal"):
        raise ValueError("Release builds require a clean checkout; commit changes before tagging")
    return tag, commit


def stage_artifact(root: Path, profile: dict[str, Any], target: str) -> Path:
    if profile.get("kind") == "packages":
        raise ValueError("No native output configured; use just publish for registry packages")
    if profile.get("kind") == "rust":
        raise ValueError("Rust packages are staged by just package")
    tag, commit = validate_release(root, profile)
    name = artifact_name(root, profile, target)
    destination = root / "build/release" / name
    if target == "flatpak":
        source = root / "build/flatpak" / f"{profile['name']}-x86_64.flatpak"
        if not source.is_file():
            raise ValueError(f"Build the Flatpak first: {source}")
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    if not destination.is_file():
        raise ValueError(f"Build the {target} release first: {destination}")
    record = {"tag": tag, "commit": commit, "target": target,
              "file": name, "sha256": digest(destination)}
    destination.with_name(name + ".build.json").write_text(
        json.dumps(record, indent=2) + "\n", encoding="utf-8")
    return destination


def release_notes(root: Path, profile: dict[str, Any], tag: str, commit: str) -> str:
    ctx = context(root, profile)
    changelog = root / ctx.changelog
    section = ""
    if changelog.is_file():
        text = changelog.read_text(encoding="utf-8")
        match = re.search(rf"^## {re.escape(tag)}(?:\s|$).*?(?=^## |\Z)", text,
                          flags=re.MULTILINE | re.DOTALL)
        if match:
            section = match[0].strip() + "\n\n"
    repository = profile["codeberg-repository"]
    notes = section + f"Source: https://codeberg.org/{repository}/src/tag/{quote(tag, safe='')}\n\nCommit: `{commit}`\n\n"
    targets = profile.get("targets", [])
    if profile.get("kind") == "rust":
        targets = ["windows"] if "windows-x86_64" in rust_project.select_unit(root, profile)["platforms"] and not ctx.source_only else []
    if "windows" in targets:
        notes += "The Windows ZIP includes the Visual C++ runtime. Extract it before running.\n"
    if "flatpak" in targets:
        notes += "Install the Flatpak with `flatpak install --user ./<downloaded-file>.flatpak`.\n"
    if ctx.source_only:
        notes += "Source archive: the whole committed repository under source/. External dependencies are not bundled.\n"
    return notes


def prepare_git_credentials(directory: Path, token: str) -> Path:
    """Create temporary build-only credentials without putting tokens in manifests."""
    directory.mkdir(parents=True, exist_ok=True)
    directory.chmod(0o700)
    store = directory / "credentials"
    store.write_text(f"https://oauth2:{quote(token, safe='')}@codeberg.org\n", encoding="utf-8")
    store.chmod(0o600)
    config = directory / "config"
    config.write_text("", encoding="utf-8")
    run("git", "config", "--file", str(config), "credential.helper", "")
    run("git", "config", "--file", str(config), "credential.https://codeberg.org.helper",
        f'store --file="{store.as_posix()}"')
    config.chmod(0o600)
    return config


def collect_artifacts(root: Path, profile: dict[str, Any]) -> tuple[list[Path], str, str]:
    if profile.get("kind") == "packages":
        raise ValueError("No native output configured; use just publish for registry packages")
    if profile.get("kind") == "rust":
        files, ctx = rust_package.collect(root, profile)
        (files[-1].parent / "release-notes.md").write_text(
            release_notes(root, profile, ctx.tag, ctx.commit), encoding="utf-8")
        return files, ctx.tag, ctx.commit
    tag, commit = validate_release(root, profile)
    directory = root / "build/release"
    targets = profile.get("targets", [name for name in ("flatpak", "windows") if name in profile])
    if not targets or len(targets) != len(set(targets)):
        raise ValueError("release.json needs distinct release targets")
    files = []
    for target in targets:
        artifact = directory / artifact_name(root, profile, target)
        record = json.loads(artifact.with_name(artifact.name + ".build.json").read_text(encoding="utf-8"))
        expected = {"tag": tag, "commit": commit, "target": target,
                    "file": artifact.name, "sha256": digest(artifact)}
        if record != expected:
            raise ValueError(f"Artifact does not match this release checkout: {artifact.name}")
        files.append(artifact)
    checksums = directory / "SHA256SUMS"
    checksums.write_text("".join(f"{digest(path)}  {path.name}\n" for path in files), encoding="utf-8")
    notes = directory / "release-notes.md"
    notes.write_text(release_notes(root, profile, tag, commit), encoding="utf-8")
    return [*files, checksums], tag, commit


class Forgejo:
    """Small Forgejo client; uploads stream from disk without extra packages."""
    def __init__(self, repository: str, token: str):
        if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
            raise ValueError("codeberg-repository must be owner/name")
        self.repository = repository
        self.token = token
        self.prefix = f"/api/v1/repos/{repository}"

    def request(self, method: str, path: str, payload: Any = None,
                missing_ok: bool = False) -> Any:
        body = json.dumps(payload).encode() if payload is not None else None
        connection = http.client.HTTPSConnection("codeberg.org", timeout=120)
        try:
            connection.request(method, path, body=body, headers={
                "Authorization": f"token {self.token}", "Content-Type": "application/json",
                "User-Agent": "monodev-release", "Accept": "application/json"})
            response = connection.getresponse()
            data = response.read()
            if missing_ok and response.status == 404:
                return None
            if not 200 <= response.status < 300:
                raise ValueError(f"Codeberg {method} {path} failed (HTTP {response.status})")
            return json.loads(data) if data else None
        finally:
            connection.close()

    def upload(self, release_id: int, path: Path) -> None:
        boundary = "monodev-" + uuid.uuid4().hex
        header = (f"--{boundary}\r\nContent-Disposition: form-data; name=\"attachment\"; "
                  f"filename=\"{path.name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").encode()
        tail = f"\r\n--{boundary}--\r\n".encode()
        endpoint = f"{self.prefix}/releases/{release_id}/assets?{urlencode({'name': path.name})}"
        connection = http.client.HTTPSConnection("codeberg.org", timeout=120)
        try:
            connection.putrequest("POST", endpoint)
            connection.putheader("Authorization", f"token {self.token}")
            connection.putheader("User-Agent", "monodev-release")
            connection.putheader("Content-Type", f"multipart/form-data; boundary={boundary}")
            connection.putheader("Content-Length", str(len(header) + path.stat().st_size + len(tail)))
            connection.endheaders()
            connection.send(header)
            with path.open("rb") as source:
                while chunk := source.read(1024 * 1024):
                    connection.send(chunk)
            connection.send(tail)
            response = connection.getresponse()
            response.read()
            if not 200 <= response.status < 300:
                raise ValueError(f"Codeberg upload of {path.name} failed (HTTP {response.status})")
        finally:
            connection.close()

    def download_checksums(self, tag: str) -> bytes:
        url = f"https://codeberg.org/{self.repository}/releases/download/{quote(tag, safe='')}/SHA256SUMS"
        for _ in range(5):
            parsed = urlsplit(url)
            if parsed.scheme != "https" or not parsed.hostname or parsed.username:
                raise ValueError("Invalid release attachment redirect")
            connection = http.client.HTTPSConnection(parsed.hostname, parsed.port, timeout=120)
            headers = {"Authorization": f"token {self.token}"} if parsed.hostname == "codeberg.org" else {}
            try:
                path = parsed.path + (f"?{parsed.query}" if parsed.query else "")
                connection.request("GET", path, headers=headers)
                response = connection.getresponse()
                data = response.read()
                if response.status in (301, 302, 303, 307, 308):
                    url = urljoin(url, response.getheader("Location", ""))
                    continue
                if response.status != 200:
                    raise ValueError(f"Cannot verify published Codeberg checksums (HTTP {response.status})")
                return data
            finally:
                connection.close()
        raise ValueError("Too many release attachment redirects")


def publish(root: Path, profile: dict[str, Any]) -> None:
    files, tag, commit = collect_artifacts(root, profile)
    repository = os.environ.get("GITHUB_REPOSITORY", "")
    token = os.environ.get("CODEBERG_TOKEN", "")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError("Set GITHUB_REPOSITORY to the GitHub mirror's owner/name")
    if profile.get("github-repository") and repository != profile["github-repository"]:
        raise ValueError("GITHUB_REPOSITORY differs from the configured release mirror")
    if not token or not os.environ.get("GH_TOKEN"):
        raise ValueError("Publishing needs GH_TOKEN and CODEBERG_TOKEN")
    if not shutil.which("gh"):
        raise ValueError("Install the GitHub CLI before publishing")
    client = Forgejo(profile["codeberg-repository"], token)
    upstream_tag = client.request("GET", f"{client.prefix}/tags/{quote(tag, safe='')}")
    if upstream_tag["commit"]["sha"] != commit:
        raise ValueError("The Codeberg tag differs from the checked-out GitHub mirror tag")
    mirror_commit = run("gh", "api", f"repos/{repository}/commits/{quote(tag, safe='')}",
                        "--jq", ".sha", stdout=subprocess.PIPE, text=True).stdout.strip()
    if mirror_commit != commit:
        raise ValueError("The GitHub mirror tag differs from the release checkout")
    codeberg = client.request("GET", f"{client.prefix}/releases/tags/{quote(tag, safe='')}", missing_ok=True)
    github_result = subprocess.run(["gh", "release", "view", tag, "--repo", repository,
                                    "--json", "isDraft,assets"], capture_output=True, text=True)
    github: dict[str, Any] | None = json.loads(github_result.stdout) if github_result.returncode == 0 else None
    checksums = files[-1].read_bytes()
    # A publish-job retry may finish a partially completed two-host publication.
    # Already published bytes are immutable; only draft assets may be replaced.
    if codeberg and not codeberg["draft"] and client.download_checksums(tag) != checksums:
        raise ValueError("Published Codeberg assets differ; create a new version instead of replacing them")
    expected_names = {path.name for path in files}
    if codeberg and not codeberg["draft"] and expected_names != {asset["name"] for asset in codeberg["assets"]}:
        raise ValueError("Published Codeberg release asset set differs; restore it before retrying")
    if github and not github["isDraft"]:
        if expected_names != {asset["name"] for asset in github["assets"]}:
            raise ValueError("Published GitHub release asset set differs; restore it before retrying")
        with tempfile.TemporaryDirectory() as directory:
            run("gh", "release", "download", tag, "--repo", repository, "--pattern", "SHA256SUMS", "--dir", directory)
            if (Path(directory) / "SHA256SUMS").read_bytes() != checksums:
                raise ValueError("Published GitHub assets differ; create a new version instead of replacing them")
    notes = files[-1].parent / "release-notes.md"
    ctx = context(root, profile)
    prerelease = "-" in ctx.version
    if codeberg is None:
        codeberg = client.request("POST", f"{client.prefix}/releases", {
            "tag_name": tag, "target_commitish": commit,
            "name": f"{ctx.name} {tag}", "body": notes.read_text(encoding="utf-8"),
            "draft": True, "prerelease": prerelease})
    if github is None:
        command = ["gh", "release", "create", tag, "--repo", repository, "--verify-tag",
                   "--draft", "--title", f"{ctx.name} {tag}", "--notes-file", str(notes)]
        if prerelease:
            command.append("--prerelease")
        run(*command)
        github = {"isDraft": True}
    if github["isDraft"] and {asset["name"] for asset in github.get("assets", [])} - expected_names:
        raise ValueError("GitHub draft has unconfigured assets; remove them before publishing")
    if codeberg["draft"]:
        existing = client.request("GET", f"{client.prefix}/releases/{codeberg['id']}/assets")
        if {asset["name"] for asset in existing} - expected_names:
            raise ValueError("Codeberg draft has unconfigured assets; remove them before publishing")
        for path in files:
            for asset in existing:
                if asset["name"] == path.name:
                    client.request("DELETE", f"{client.prefix}/releases/{codeberg['id']}/assets/{asset['id']}")
            client.upload(codeberg["id"], path)
    if github["isDraft"]:
        run("gh", "release", "upload", tag, "--repo", repository, "--clobber", *(str(path) for path in files))
    # Both hosts now have every required asset before either new draft goes live.
    if codeberg["draft"]:
        client.request("PATCH", f"{client.prefix}/releases/{codeberg['id']}", {"draft": False})
    if github["isDraft"]:
        run("gh", "release", "edit", tag, "--repo", repository, "--draft=false")
    print(f"Published {tag} on GitHub and Codeberg")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", default="packaging/release.json")
    commands = parser.add_subparsers(dest="command", required=True)
    check = commands.add_parser("check")
    check.add_argument("tag", nargs="?", default="")
    commands.add_parser("info")
    commands.add_parser("auth", help="Prepare optional temporary credentials for private Codeberg dependencies")
    commands.add_parser("auth-clean", help="Remove temporary build credentials")
    stage = commands.add_parser("stage")
    stage.add_argument("target", choices=("flatpak", "windows"))
    commands.add_parser("collect")
    commands.add_parser("publish")
    args = parser.parse_args(argv)
    try:
        profile = load_profile(ROOT, args.profile)
        if args.command == "check":
            try:
                tag, commit = validate_release(ROOT, profile, args.tag)
            except ValueError:
                if profile.get("kind") == "packages" or not registry.configured(profile):
                    raise
                tag, commit = registry.check_tag(ROOT, profile, requested_tag(args.tag))
            print(f"Validated {tag} at {commit}")
        elif args.command == "info":
            if profile.get("kind") == "packages":
                packages = []
                for name in registry.configured(profile):
                    entries, data = registry.metadata_for(ROOT, profile, name)
                    packages.extend({"registry": name, "name": data[e["manifest"]]["name"],
                                     "version": data[e["manifest"]]["version"], "manifest": e["manifest"]} for e in entries)
                info = {"outputs": json.dumps(registry.configured(profile)), "packages": json.dumps(packages)}
                if requested_tag():
                    tag, commit = registry.check_tag(ROOT, profile, requested_tag())
                    info.update(tag=tag, commit=commit)
            elif profile.get("kind") == "rust":
                ctx = context(ROOT, profile)
                unit = rust_project.select_unit(ROOT, profile)
                matrix = [{"target": p, "os": rust_project.PLATFORMS[p]} for p in unit["platforms"]]
                info = {"rust-toolchain": profile["rust-toolchain"], "version": ctx.version, "tag": ctx.tag,
                        "unit": unit["id"], "matrix": json.dumps({"include": matrix}, separators=(",", ":"))}
            else:
                if profile.get("registries") and requested_tag():
                    validate_release(ROOT, profile)
                info = {"flutter-version": profile["flutter-version"], "version": app_version(ROOT),
                    "tag": release_tag(ROOT, profile), "rust": str(profile.get("rust", False)).lower()}
            info["outputs"] = json.dumps((["native"] if profile.get("kind") != "packages" else []) + registry.configured(profile))
            if profile.get("registries"):
                info["registries"] = json.dumps(profile["registries"])
            print(json.dumps(info))
            if os.environ.get("GITHUB_OUTPUT"):
                with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
                    output.write("".join(f"{key}={value}\n" for key, value in info.items()))
        elif args.command == "auth":
            token = os.environ.get("CODEBERG_READ_TOKEN", "")
            if token:
                directory = Path(os.environ["RUNNER_TEMP"]) / "monodev-codeberg-auth"
                config = prepare_git_credentials(directory, token)
                with Path(os.environ["GITHUB_ENV"]).open("a", encoding="utf-8") as output:
                    output.write(f"GIT_CONFIG_GLOBAL={config}\nMONODEV_GIT_CONFIG={config}\nCARGO_NET_GIT_FETCH_WITH_CLI=true\n")
                print("Prepared temporary Codeberg dependency credentials")
        elif args.command == "auth-clean":
            shutil.rmtree(Path(os.environ["RUNNER_TEMP"]) / "monodev-codeberg-auth", ignore_errors=True)
        elif args.command == "stage":
            print(stage_artifact(ROOT, profile, args.target))
        elif args.command == "collect":
            files, _, _ = collect_artifacts(ROOT, profile)
            print("\n".join(str(path) for path in files))
        else:
            publish(ROOT, profile)
        return 0
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError, http.client.HTTPException) as error:
        print(f"Release failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
