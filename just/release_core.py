"""Shared release identity, source validation and safe archive paths."""
from __future__ import annotations

from dataclasses import dataclass
import hashlib
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
from typing import Any

ROOT = Path(__file__).resolve().parent.parent


def run(*args: str, **kwargs: Any) -> subprocess.CompletedProcess[Any]:
    return subprocess.run(args, check=True, **kwargs)


def git(root: Path, *args: str) -> str:
    return run("git", *args, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True).stdout.strip()


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def safe_name(value: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", value):
        raise ValueError(f"Expected a plain release name: {value!r}")
    return value


def relative_path(value: str) -> str:
    if not isinstance(value, str) or not value or "\\" in value or ":" in value:
        raise ValueError(f"Expected a relative path: {value!r}")
    path = PurePosixPath(value)
    if path.is_absolute() or any(part in ("..", ".git") for part in path.parts):
        raise ValueError(f"Path escapes the release checkout: {value}")
    if str(path) == ".":
        raise ValueError("Expected a file path")
    return path.as_posix()


def add_destination(names: set[str], name: str) -> None:
    key = relative_path(name).casefold()
    if key in names or any(key.startswith(n + "/") or n.startswith(key + "/") for n in names):
        raise ValueError(f"Conflicting archive destination: {name}")
    names.add(key)


def project_path(root: Path, value: str, *, exists: bool = True) -> Path:
    relative = relative_path(value)
    path = root
    for part in PurePosixPath(relative).parts:
        path /= part
        if path.is_symlink():
            raise ValueError(f"Symlinked release path: {value}")
    if not path.resolve().is_relative_to(root.resolve()):
        raise ValueError(f"Path escapes the release checkout: {value}")
    if exists and not path.is_file():
        raise ValueError(f"Missing release file: {value}")
    return path


def requested_tag(explicit: str = "") -> str:
    return explicit or os.environ.get("RELEASE_TAG") or os.environ.get("GITHUB_REF_NAME", "")


def clean_tag(root: Path, tag: str) -> str:
    if not tag or subprocess.run(["git", "check-ref-format", f"refs/tags/{tag}"],
                                 capture_output=True).returncode:
        raise ValueError(f"Invalid release tag: {tag!r}")
    commit = git(root, "rev-parse", "HEAD")
    if git(root, "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}") != commit:
        raise ValueError("Check out the release tag itself before building or publishing")
    if git(root, "status", "--porcelain", "--untracked-files=normal"):
        raise ValueError("Release builds require a clean checkout; commit changes before tagging")
    return commit


@dataclass(frozen=True)
class ReleaseContext:
    """One validated release unit; no SDK or build-tool requirements."""
    name: str
    version: str
    tag: str
    commit: str
    changelog: str
    artifacts: tuple[str, ...]
    source_only: bool = False
