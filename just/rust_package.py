"""Build and test one native Rust release unit and collect portable archives.

Cargo supplies executable paths. Libraries distribute the committed repository
source only after every selected platform has supplied a matching test receipt.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tarfile
from typing import Any
import zipfile

# Release validation must not create untracked bytecode in consumer checkouts.
if __name__ == "__main__":
    sys.dont_write_bytecode = True

from native import find_windows_crt
from release_core import ROOT, ReleaseContext, add_destination, digest, project_path, relative_path
import rust_project as rust


def directory(root: Path, unit: dict[str, Any], ctx: ReleaseContext) -> Path:
    return project_path(root, f"build/release/{unit['id']}/{ctx.version}/placeholder", exists=False).parent


def host_platform() -> str:
    machine = platform.machine().lower()
    if machine not in ("x86_64", "amd64") or sys.platform not in ("linux", "win32"):
        raise ValueError("Native packaging supports Linux x86_64 and Windows x86_64")
    return "windows-x86_64" if sys.platform == "win32" else "linux-x86_64"


def package_args(package: dict[str, Any], entry: dict[str, Any]) -> list[str]:
    args = ["--manifest-path", package["manifest_path"], "--package", package["name"], "--locked"]
    if not entry["default-features"]:
        args.append("--no-default-features")
    if entry["features"]:
        args.extend(["--features", ",".join(entry["features"])])
    return args


def archive_files(root: Path, unit: dict[str, Any], target: str,
                  executables: dict[str, Path], crt: Path | None = None) -> dict[str, Path]:
    files: dict[str, Path] = {}
    names: set[str] = set()

    def add(name: str, path: Path) -> None:
        name = relative_path(name)
        add_destination(names, name)
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"Missing or symlinked archive file: {path}")
        files[name] = path

    for name, path in executables.items():
        add(name + (".exe" if target.startswith("windows") else ""), path)
    for name in unit.get("licenses", []):
        add("licenses/" + relative_path(name), project_path(root, name))
    for asset in unit.get("assets", []):
        if target in asset.get("platforms", unit["platforms"]):
            add(asset["destination"], project_path(root, asset["path"]))
    if target.startswith("windows"):
        runtime = crt or find_windows_crt()
        for name in ("msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"):
            if not (runtime / name).is_file():
                raise ValueError(f"Incomplete Windows C++ runtime: {name}")
        for path in runtime.glob("*.dll"):
            add(path.name, path)
    return files


def write_archive(destination: Path, files: dict[str, Path]) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_name(destination.name + ".partial")
    try:
        if destination.suffix == ".zip":
            with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                for name, path in sorted(files.items()):
                    archive.write(path, name)
        else:
            with tarfile.open(temporary, "w:gz") as archive:
                for name, path in sorted(files.items()):
                    archive.add(path, arcname=name, recursive=False)
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)


def record(ctx: ReleaseContext, unit: dict[str, Any], target: str, path: Path | None = None) -> dict[str, Any]:
    value: dict[str, Any] = {"unit": unit["id"], "tag": ctx.tag, "commit": ctx.commit, "target": target}
    if path:
        value.update(file=path.name, sha256=digest(path))
    return value


def build(root: Path, profile: dict[str, Any], selector: str = "", target: str = "") -> Path:
    if profile.get("kind") == "packages":
        raise ValueError("No native output configured; use just publish for registry packages")
    packages = rust.validate_profile(root, profile)
    unit = rust.select_unit(root, profile, selector)
    rust.committed_locks(root, packages, [p["manifest"] for p in unit["packages"]])
    ignored = subprocess.run(["git", "check-ignore", "--quiet", "--no-index", "build/release/probe"], cwd=root)
    if ignored.returncode:
        raise ValueError("Ignore build/release/ in .gitignore before packaging a clean release")
    ctx = rust.context(root, profile, selector)
    target = target or host_platform()
    if target != host_platform() or target not in unit["platforms"]:
        raise ValueError("Choose a configured platform matching the native runner")
    # Reject colliding source destinations before compiling or writing output.
    placeholders = {b: project_path(root, p["manifest"]) for p in unit["packages"] for b in p["binaries"]}
    archive_files(root, unit, target, placeholders)
    executables: dict[str, Path] = {}
    for entry in unit["packages"]:
        package = packages[entry["manifest"]]
        arguments = package_args(package, entry)
        rust.cargo(root, profile["rust-toolchain"], "test", *arguments)
        for binary in entry["binaries"]:
            result = rust.cargo(root, profile["rust-toolchain"], "build", "--release", *arguments,
                                "--bin", binary, "--message-format=json", stdout=subprocess.PIPE, text=True)
            matches: list[Path] = []
            for line in result.stdout.splitlines():
                message = json.loads(line)
                if message.get("reason") == "compiler-artifact" and message.get("package_id") == package["id"]:
                    if message["target"]["name"] == binary and message.get("executable"):
                        matches.append(Path(message["executable"]))
                elif message.get("reason") == "compiler-message":
                    print(message["message"].get("rendered", ""), file=sys.stderr)
            if len(matches) != 1:
                raise ValueError(f"Cargo did not report exactly one executable for {binary}")
            executables[binary] = matches[0]
    # A build script must not change the source under the release tag.
    rust.context(root, profile, selector)
    output = directory(root, unit, ctx)
    output.mkdir(parents=True, exist_ok=True)
    if executables:
        index = unit["platforms"].index(target)
        destination = output / ctx.artifacts[index]
        write_archive(destination, archive_files(root, unit, target, executables))
        data = record(ctx, unit, target, destination)
    else:
        destination = output / f"{target}.tests.json"
        data = record(ctx, unit, target)
    receipt = (destination.with_name(destination.name + ".build.json") if executables else destination)
    receipt.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    return destination


def source_archive(root: Path, ctx: ReleaseContext, destination: Path) -> None:
    # Validate committed symlinks and gitlinks before exporting a repository tree.
    result = subprocess.run(["git", "ls-tree", "-r", "-z", ctx.commit], cwd=root, check=True, capture_output=True)
    for entry in result.stdout.split(b"\0"):
        if not entry:
            continue
        header, name = entry.split(b"\t", 1)
        if header.startswith((b"120000", b"160000")):
            raise ValueError(f"Source archive needs a regular committed tree; symlink/submodule: {name.decode()}")
    temporary = destination.with_name(destination.name + ".partial")
    try:
        subprocess.run(["git", "archive", "--format=tar.gz", "--prefix=source/", "--output", str(temporary),
                        ctx.commit], cwd=root, check=True)
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)


def collect(root: Path, profile: dict[str, Any]) -> tuple[list[Path], ReleaseContext]:
    ctx = rust.context(root, profile)
    unit = rust.select_unit(root, profile)
    output = directory(root, unit, ctx)
    expected = set(ctx.artifacts) | {"SHA256SUMS", "release-notes.md"}
    if ctx.source_only:
        expected |= {f"{p}.tests.json" for p in unit["platforms"]}
    else:
        expected |= {name + ".build.json" for name in ctx.artifacts}
    if not output.is_dir():
        raise ValueError("Build every configured platform before collecting")
    if any(p.name not in expected or p.is_symlink() or not p.is_file() for p in output.iterdir()):
        raise ValueError("Unexpected or mixed-unit release artifacts")
    if ctx.source_only:
        for target in unit["platforms"]:
            receipt = json.loads((output / f"{target}.tests.json").read_text())
            if receipt != record(ctx, unit, target):
                raise ValueError("Library tests do not match this release unit/tag/commit")
        source_archive(root, ctx, output / ctx.artifacts[0])
    else:
        for target, name in zip(unit["platforms"], ctx.artifacts, strict=True):
            artifact = output / name
            receipt = json.loads((output / (name + ".build.json")).read_text())
            if receipt != record(ctx, unit, target, artifact):
                raise ValueError("Artifact does not match this release unit/tag/commit")
    files = [output / name for name in ctx.artifacts]
    checksums = output / "SHA256SUMS"
    checksums.write_text("".join(f"{digest(path)}  {path.name}\n" for path in files), encoding="utf-8")
    return [*files, checksums], ctx


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", default="packaging/release.json")
    parser.add_argument("--unit", default="")
    parser.add_argument("--non-interactive", action="store_true")
    parser.add_argument("--target", choices=tuple(rust.PLATFORMS), default="")
    args = parser.parse_args(argv)
    try:
        profile = json.loads(project_path(ROOT, args.profile).read_text())
        if not args.unit and not rust.requested_tag() and len(profile.get("releases", [])) > 1:
            ci = os.environ.get("CI", "").lower() not in ("", "false", "0", "no")
            if sys.stdin.isatty() and sys.stdout.isatty() and not ci and not args.non_interactive:
                choices = [u["id"] for u in profile["releases"]]
                print("Release units: " + ", ".join(choices))
                args.unit = input("Release unit: ").strip()
        print(build(ROOT, profile, args.unit, args.target))
        return 0
    except (EOFError, KeyboardInterrupt):
        print("Packaging cancelled.", file=sys.stderr)
        return 130
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        print(f"Rust packaging failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
