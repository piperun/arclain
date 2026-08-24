#!/usr/bin/env python3
"""Enforce Arclain's exact external Wirt dependency boundary."""
from __future__ import annotations

import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any, Iterable


REPO_ROOT = Path(__file__).resolve().parent.parent
CANONICAL_GIT_URL = "https://codeberg.org/0xdev/wirt.git"
REVISION_RE = re.compile(r"^[0-9a-f]{40}$")
VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$")
WIRT_PACKAGES = frozenset({"wirt", "wirt-cli", "wirt-sdk"})
EMBEDDED_MEMBERS = frozenset({"crates/wirt", "crates/wirt-cli", "wirt-sdk"})
EMBEDDED_DIRECTORIES = (
    ("crates/wirt", "embedded Wirt source remains"),
    ("crates/wirt-cli", "embedded Wirt CLI source remains"),
    ("wirt-sdk", "embedded Wirt SDK remains"),
)
GUEST_LOCKS = (
    "plugins/dlsite-metadata/Cargo.lock",
    "plugins/facade-test-fixture/Cargo.lock",
    "plugins/gstreamer-preview/Cargo.lock",
    "plugins/ui-demo/Cargo.lock",
    "crates/plugins/tests/fixtures/failing-init/Cargo.lock",
    "crates/plugins/tests/fixtures/malicious-metadata/Cargo.lock",
)
GUEST_MANIFESTS = tuple(path.replace("Cargo.lock", "Cargo.toml") for path in GUEST_LOCKS)
REMOVED_FIXTURE_PATH = "crates/wirt" + "/tests/fixtures/bundled"
IGNORED_PARTS = frozenset({".git", "target", ".tmp"})
DEFAULT_TREE = ["cargo", "tree", "-p", "arclain_app", "-e", "normal"]
ARCHIVE_ONLY_TREE = [
    "cargo",
    "tree",
    "-p",
    "arclain_app",
    "-e",
    "normal",
    "--no-default-features",
]
TREE_GLYPHS = "│├└─ \t"


def _relative(root: Path, path: Path) -> str:
    return path.relative_to(root).as_posix()


def _load_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def _toolchain(root: Path, errors: list[str]) -> tuple[str, str] | None:
    initial_error_count = len(errors)
    path = root / "wirt-toolchain.toml"
    if not path.is_file():
        errors.append("wirt-toolchain.toml: Wirt toolchain pin is missing")
        return None
    try:
        document = _load_toml(path)
    except (OSError, tomllib.TOMLDecodeError) as error:
        errors.append(f"wirt-toolchain.toml: invalid TOML: {error}")
        return None

    table = document.get("wirt")
    expected_keys = {"git", "rev", "cli_version", "abi"}
    if not isinstance(table, dict) or set(table) != expected_keys:
        errors.append(
            "wirt-toolchain.toml: [wirt] must contain exactly git, rev, "
            "cli_version, and abi"
        )
        return None

    git_url = table.get("git")
    revision = table.get("rev")
    cli_version = table.get("cli_version")
    abi = table.get("abi")
    if git_url != CANONICAL_GIT_URL:
        errors.append("wirt-toolchain.toml: Wirt Git URL is not canonical")
    if not isinstance(revision, str) or not REVISION_RE.fullmatch(revision):
        errors.append("wirt-toolchain.toml: Wirt revision is not exact")
    if not isinstance(cli_version, str) or not VERSION_RE.fullmatch(cli_version):
        errors.append("wirt-toolchain.toml: Wirt CLI version is not exact")
    if not isinstance(abi, str) or not VERSION_RE.fullmatch(abi):
        errors.append("wirt-toolchain.toml: Wirt ABI version is not exact")
    if len(errors) != initial_error_count:
        return None
    return git_url, revision


def _workspace_members(root: Path, errors: list[str]) -> None:
    manifest_path = root / "Cargo.toml"
    try:
        manifest = _load_toml(manifest_path)
    except (OSError, tomllib.TOMLDecodeError) as error:
        errors.append(f"Cargo.toml: invalid TOML: {error}")
        return
    workspace = manifest.get("workspace", {})
    members = workspace.get("members", []) if isinstance(workspace, dict) else []
    for member in sorted(EMBEDDED_MEMBERS.intersection(members)):
        errors.append(f"Cargo.toml: embedded Wirt workspace member {member}")


def _dependency_tables(manifest: dict[str, Any]) -> Iterable[dict[str, Any]]:
    for table_name in ("dependencies", "dev-dependencies", "build-dependencies"):
        table = manifest.get(table_name)
        if isinstance(table, dict):
            yield table

    workspace = manifest.get("workspace")
    if isinstance(workspace, dict):
        dependencies = workspace.get("dependencies")
        if isinstance(dependencies, dict):
            yield dependencies

    targets = manifest.get("target")
    if isinstance(targets, dict):
        for target in targets.values():
            if not isinstance(target, dict):
                continue
            for table_name in (
                "dependencies",
                "dev-dependencies",
                "build-dependencies",
            ):
                table = target.get(table_name)
                if isinstance(table, dict):
                    yield table


def _is_wirt_dependency(name: str, specification: Any) -> bool:
    return name in WIRT_PACKAGES or (
        isinstance(specification, dict)
        and specification.get("package") in WIRT_PACKAGES
    )


def _is_exact_dependency(
    name: str,
    specification: Any,
    *,
    git_url: str,
    revision: str,
    allow_workspace: bool,
) -> bool:
    if not isinstance(specification, dict):
        return False
    if specification.get("workspace") is True:
        return allow_workspace and name == "wirt" and not any(
            key in specification for key in ("git", "rev", "path", "branch", "tag")
        )
    return (
        specification.get("git") == git_url
        and specification.get("rev") == revision
        and "path" not in specification
        and "branch" not in specification
        and "tag" not in specification
    )


def _workspace_member_manifests(root: Path) -> set[Path]:
    try:
        manifest = _load_toml(root / "Cargo.toml")
    except (OSError, tomllib.TOMLDecodeError):
        return set()
    workspace = manifest.get("workspace", {})
    members = workspace.get("members", []) if isinstance(workspace, dict) else []
    return {
        (root / member / "Cargo.toml").resolve()
        for member in members
        if isinstance(member, str)
    }


def _manifest_dependencies(
    root: Path,
    errors: list[str],
    *,
    git_url: str,
    revision: str,
) -> None:
    workspace_members = _workspace_member_manifests(root)
    manifests = sorted(
        path
        for path in root.rglob("Cargo.toml")
        if not IGNORED_PARTS.intersection(path.relative_to(root).parts)
    )
    for path in manifests:
        try:
            manifest = _load_toml(path)
        except (OSError, tomllib.TOMLDecodeError) as error:
            errors.append(f"{_relative(root, path)}: invalid TOML: {error}")
            continue
        invalid = False
        is_app_manifest = path.relative_to(root).as_posix() == "crates/app/Cargo.toml"
        app_wirt_dependencies: list[Any] = []
        normal_dependencies = manifest.get("dependencies")
        has_normal_app_wirt = (
            is_app_manifest
            and isinstance(normal_dependencies, dict)
            and "wirt" in normal_dependencies
        )
        normal_app_wirt = (
            normal_dependencies["wirt"] if has_normal_app_wirt else None
        )
        for table in _dependency_tables(manifest):
            for name, specification in table.items():
                if _is_wirt_dependency(name, specification):
                    exact = _is_exact_dependency(
                        name,
                        specification,
                        git_url=git_url,
                        revision=revision,
                        allow_workspace=path.resolve() in workspace_members,
                    )
                    if not exact:
                        invalid = True
                    if is_app_manifest and name == "wirt":
                        app_wirt_dependencies.append(specification)
        if invalid:
            errors.append(
                f"{_relative(root, path)}: Wirt dependency is not an exact Git revision"
            )
        if is_app_manifest:
            if not has_normal_app_wirt:
                errors.append(
                    "crates/app/Cargo.toml: required direct wirt dependency is missing"
                )
            else:
                normal_app_wirt_is_exact = _is_exact_dependency(
                    "wirt",
                    normal_app_wirt,
                    git_url=git_url,
                    revision=revision,
                    allow_workspace=path.resolve() in workspace_members,
                )
                if (
                    not isinstance(normal_app_wirt, dict)
                    or normal_app_wirt.get("optional") is not True
                ):
                    errors.append("crates/app/Cargo.toml: wirt dependency must be optional")
                if normal_app_wirt_is_exact and normal_app_wirt.get("workspace") is not True:
                    errors.append(
                        "crates/app/Cargo.toml: wirt dependency must inherit the workspace pin"
                    )
            if len(app_wirt_dependencies) > 1:
                errors.append(
                    "crates/app/Cargo.toml: expected exactly one direct wirt dependency, "
                    f"found {len(app_wirt_dependencies)}"
                )


def wirt_tree_errors(default_tree: str, archive_only_tree: str) -> list[str]:
    """Require Wirt in the default graph and exclude it from archive-only."""
    errors: list[str] = []
    if "wirt" not in cargo_tree_package_names(default_tree):
        errors.append("default arclain_app tree is missing Wirt")
    if "wirt" in cargo_tree_package_names(archive_only_tree):
        errors.append("archive-only arclain_app tree contains Wirt")
    return errors


def cargo_tree_package_names(tree: str) -> set[str]:
    """Parse exact package-name tokens from rendered `cargo tree` lines."""
    packages: set[str] = set()
    for line in tree.splitlines():
        crate_line = line.lstrip(TREE_GLYPHS)
        if crate_line:
            packages.add(crate_line.split(maxsplit=1)[0])
    return packages


def _cargo_tree(command: list[str]) -> str | None:
    result = subprocess.run(
        command,
        cwd=REPO_ROOT,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        print(f"Wirt dependency boundary: `{' '.join(command)}` failed", file=sys.stderr)
        return None
    return result.stdout


def _required_dependencies(root: Path, errors: list[str]) -> None:
    try:
        root_manifest = _load_toml(root / "Cargo.toml")
    except (OSError, tomllib.TOMLDecodeError):
        root_manifest = {}
    workspace = root_manifest.get("workspace", {})
    workspace_dependencies = (
        workspace.get("dependencies", {}) if isinstance(workspace, dict) else {}
    )
    if not isinstance(workspace_dependencies, dict) or "wirt" not in workspace_dependencies:
        errors.append("Cargo.toml: required Wirt workspace dependency is missing")

    for relative in GUEST_MANIFESTS:
        path = root / relative
        try:
            manifest = _load_toml(path)
        except (OSError, tomllib.TOMLDecodeError):
            continue
        dependencies = manifest.get("dependencies", {})
        if not isinstance(dependencies, dict) or "wirt-sdk" not in dependencies:
            errors.append(f"{relative}: required wirt-sdk dependency is missing")


def _embedded_directories(root: Path, errors: list[str]) -> None:
    for relative, message in EMBEDDED_DIRECTORIES:
        if (root / relative).exists():
            errors.append(f"{relative}: {message}")


def _duplicate_wit(root: Path, errors: list[str]) -> None:
    declaration = re.compile(r"^\s*package\s+wirt:plugin@", re.MULTILINE)
    for path in sorted(root.rglob("*.wit")):
        if IGNORED_PARTS.intersection(path.relative_to(root).parts):
            continue
        try:
            content = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        if declaration.search(content):
            errors.append(
                f"{_relative(root, path)}: duplicate local Wirt package remains"
            )


def _tracked_paths(root: Path) -> list[Path]:
    if (root / ".git").exists():
        result = subprocess.run(
            ["git", "-C", str(root), "ls-files", "-z"],
            capture_output=True,
            check=False,
        )
        if result.returncode == 0:
            return [
                root / relative.decode("utf-8")
                for relative in result.stdout.split(b"\0")
                if relative
            ]
    return [
        path
        for path in root.rglob("*")
        if path.is_file()
        and not IGNORED_PARTS.intersection(path.relative_to(root).parts)
    ]


def _removed_fixture_references(root: Path, errors: list[str]) -> None:
    for path in sorted(_tracked_paths(root)):
        if not path.is_file():
            continue
        try:
            content = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        if REMOVED_FIXTURE_PATH in content or REMOVED_FIXTURE_PATH.replace(
            "/", "\\"
        ) in content:
            errors.append(f"{_relative(root, path)}: removed Wirt fixture path remains")


def _expected_lock_source(git_url: str, revision: str) -> str:
    return f"git+{git_url}?rev={revision}#{revision}"


def _guest_locks(
    root: Path,
    errors: list[str],
    *,
    git_url: str,
    revision: str,
) -> None:
    expected_source = _expected_lock_source(git_url, revision)
    for relative in GUEST_LOCKS:
        path = root / relative
        if not path.is_file():
            errors.append(f"{relative}: guest lockfile is missing")
            continue
        try:
            lock = _load_toml(path)
        except (OSError, tomllib.TOMLDecodeError) as error:
            errors.append(f"{relative}: invalid TOML: {error}")
            continue
        sources = [
            package.get("source")
            for package in lock.get("package", [])
            if isinstance(package, dict) and package.get("name") == "wirt-sdk"
        ]
        if sources != [expected_source]:
            errors.append(
                f"{relative}: wirt-sdk lock source is not the exact Git revision"
            )


def _root_lock(
    root: Path,
    errors: list[str],
    *,
    git_url: str,
    revision: str,
) -> None:
    path = root / "Cargo.lock"
    if not path.is_file():
        return
    try:
        lock = _load_toml(path)
    except (OSError, tomllib.TOMLDecodeError) as error:
        errors.append(f"Cargo.lock: invalid TOML: {error}")
        return
    expected_source = _expected_lock_source(git_url, revision)
    for package in lock.get("package", []):
        if not isinstance(package, dict) or package.get("name") not in WIRT_PACKAGES:
            continue
        if package.get("source") != expected_source:
            errors.append(
                f"Cargo.lock: {package.get('name')} source is not the exact Git revision"
            )


def check(root: Path = REPO_ROOT) -> list[str]:
    root = root.resolve()
    errors: list[str] = []
    _workspace_members(root, errors)
    pins = _toolchain(root, errors)
    if pins is not None:
        git_url, revision = pins
        _required_dependencies(root, errors)
        _manifest_dependencies(
            root,
            errors,
            git_url=git_url,
            revision=revision,
        )
    _embedded_directories(root, errors)
    _duplicate_wit(root, errors)
    _removed_fixture_references(root, errors)
    if pins is not None:
        _root_lock(root, errors, git_url=git_url, revision=revision)
        _guest_locks(root, errors, git_url=git_url, revision=revision)
    return errors


def main() -> int:
    errors = check()
    default_tree = _cargo_tree(DEFAULT_TREE)
    archive_only_tree = _cargo_tree(ARCHIVE_ONLY_TREE)
    if default_tree is None or archive_only_tree is None:
        return 1
    errors.extend(wirt_tree_errors(default_tree, archive_only_tree))
    if errors:
        print("Wirt dependency boundary violations:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    print("Wirt dependency boundary: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
