#!/usr/bin/env python3
"""Flutter desktop packaging shared by local just recipes and CI (stdlib only)."""
from __future__ import annotations

import argparse
import copy
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
from typing import Any

# Release validation must not create untracked bytecode in consumer checkouts.
if __name__ == "__main__":
    sys.dont_write_bytecode = True

from native import find_windows_crt
from urllib.parse import unquote, urljoin, urlparse
import zipfile


ROOT = Path(__file__).resolve().parent.parent


def run(*args: str, **kwargs: Any) -> subprocess.CompletedProcess[Any]:
    return subprocess.run(args, check=True, **kwargs)


def load_profile(root: Path, path: str = "packaging/release.json") -> dict[str, Any]:
    profile_path = root / path
    if not profile_path.is_file():
        raise ValueError(f"Missing {profile_path}. See monodev/docs/flutter-releases.md")
    profile = json.loads(profile_path.read_text(encoding="utf-8"))
    if profile.get("kind") == "packages":
        raise ValueError("No native output configured; use just publish for registry packages")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", profile.get("name", "")):
        raise ValueError("release.json needs a filename-safe name, e.g. MyApp")
    if not re.fullmatch(r"\d+\.\d+\.\d+", profile.get("flutter-version", "")):
        raise ValueError("release.json needs an exact flutter-version")
    if not re.fullmatch(r"[A-Za-z0-9._-]*", profile.get("tag-prefix", "")):
        raise ValueError("tag-prefix must be filename-safe (for example v or myapp-v)")
    return profile


def app_version(root: Path) -> str:
    text = (root / "pubspec.yaml").read_text(encoding="utf-8")
    match = re.search(r"^version:\s*['\"]?([^\s'\"#]+)", text, re.MULTILINE)
    version = match[1].split("+", 1)[0] if match else ""
    if not re.fullmatch(r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?", version):
        raise ValueError("pubspec.yaml needs a SemVer version (managed by your versioning tool)")
    return version


def release_tag(root: Path, profile: dict[str, Any]) -> str:
    return profile.get("tag-prefix", "") + app_version(root)


def artifact_name(root: Path, profile: dict[str, Any], target: str) -> str:
    suffix = {"flatpak": "linux-x86_64.flatpak", "windows": "windows-x86_64.zip"}
    return f"{profile['name']}-{release_tag(root, profile)}-{suffix[target]}"


def stage_source(root: Path, destination: Path) -> None:
    """Snapshot Git-visible working files without ignored secrets/build outputs."""
    files = run("git", "ls-files", "--cached", "--others", "--exclude-standard", "-z",
                cwd=root, stdout=subprocess.PIPE).stdout.decode().split("\0")
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    for name in files:
        source = root / name
        if not name or not source.is_file():
            continue
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)

    # Pub's per-checkout overrides remain useful for local dependency work.
    # Release validation rejects them before CI starts building.
    overrides = root / "pubspec_overrides.yaml"
    if overrides.exists():
        config = root / ".dart_tool/package_config.json"
        if not config.exists():
            raise ValueError("Run flutter pub get before building with local overrides")
        for package in json.loads(config.read_text(encoding="utf-8"))["packages"]:
            uri = urlparse(urljoin(config.as_uri(), package["rootUri"]))
            package_root = Path(unquote(uri.path)).resolve()
            if package_root == root or not package_root.is_relative_to(root):
                continue
            target = destination / package_root.relative_to(root)
            if not target.exists():
                shutil.copytree(package_root, target, ignore=shutil.ignore_patterns(
                    ".git", ".dart_tool", "build", "target", "__pycache__"))
        shutil.copy2(overrides, destination / overrides.name)


def configure_manifest(template: dict[str, Any], source: Path, flutter: Path,
                       pub: Path, module_name: str, manifest_dir: Path,
                       rust_paths: tuple[Path, Path, Path] | None = None,
                       jobs: int | None = None) -> dict[str, Any]:
    manifest = copy.deepcopy(template)
    modules = manifest["modules"]
    matches = [module for module in modules if isinstance(module, dict)
               and module.get("name") == module_name]
    if len(matches) != 1:
        raise ValueError(f"Expected one top-level application module named {module_name}")

    def resolve_sources(module: dict[str, Any]) -> None:
        for item in module.get("sources", []):
            if "path" in item:
                item["path"] = str((manifest_dir / item["path"]).resolve())
        for child in module.get("modules", []):
            if isinstance(child, dict):
                resolve_sources(child)

    for i, module in enumerate(modules):
        if isinstance(module, str):
            modules[i] = str((manifest_dir / module).resolve())
        else:
            resolve_sources(module)
    matches[0]["sources"] = [{"type": "dir", "path": str(source)}]
    options = manifest.setdefault("build-options", {})
    env = options.setdefault("env", {})
    env.update(FLUTTER_ROOT=str(flutter), PUB_CACHE=str(pub))
    paths = [flutter, pub]
    if rust_paths:
        cargo, rustup, original_cargo = rust_paths
        env.update(CARGO_HOME=str(cargo), RUSTUP_HOME=str(rustup))
        paths.extend(rust_paths)
    if jobs is not None:
        env.update(CARGO_BUILD_JOBS=str(jobs), CMAKE_BUILD_PARALLEL_LEVEL=str(jobs))
    options.setdefault("build-args", []).extend(f"--filesystem={path}" for path in paths)
    return manifest


def find_flutter(sdk: Path | None = None) -> Path:
    command = shutil.which("flutter")
    selected = sdk or os.environ.get("FLUTTER_ROOT")
    if not selected and command:
        selected = Path(command).resolve().parent.parent
    if selected and not sdk and not os.environ.get("FLUTTER_ROOT") and not (Path(selected) / "packages/flutter_tools").is_dir():
        selected = None  # A PATH entry may be a launcher wrapper, not the SDK.
    if not selected and (Path.home() / "flutter/bin/flutter").exists():
        selected = Path.home() / "flutter"
    if not selected or not (Path(selected) / "packages/flutter_tools").is_dir():
        raise ValueError("Set FLUTTER_ROOT to a Flutter SDK, or use --flutter-sdk PATH")
    return Path(selected).resolve()


def build_flatpak(root: Path, profile: dict[str, Any], args: argparse.Namespace) -> None:
    if sys.platform != "linux":
        raise ValueError("Flatpak builds require Linux")
    if not shutil.which("flatpak"):
        raise ValueError("Install Flatpak on the host, then run just flatpak there")
    settings = profile["flatpak"]
    template_path = root / settings["manifest"]
    template = json.loads(template_path.read_text(encoding="utf-8"))
    flutter = find_flutter(args.flutter_sdk)
    build = root / "build/flatpak"
    build.mkdir(parents=True, exist_ok=True)
    pub = Path(os.environ.get("PUB_CACHE", Path.home() / ".pub-cache")).resolve()
    pub.mkdir(parents=True, exist_ok=True)
    builder = args.builder
    if builder == "auto":
        builder = "native" if shutil.which("flatpak-builder") else "app"
    if builder == "native" and not shutil.which("flatpak-builder"):
        raise ValueError("Install flatpak-builder, or select --builder app")
    branch = template["runtime-version"]
    refs = [f"{template['runtime']}//{branch}", f"{template['sdk']}//{branch}",
            *(f"{extension}//{branch}" for extension in template.get("sdk-extensions", []))]
    if builder == "app":
        refs.append("org.flatpak.Builder")
    for ref in refs:
        result = subprocess.run(["flatpak", "info", ref], stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL)
        if result.returncode:
            raise ValueError(f"Missing {ref}. Install it with: flatpak install flathub {ref}")

    rust_paths = None
    if profile.get("rust", False):
        rustup = Path(os.environ.get("RUSTUP_HOME", Path.home() / ".rustup")).resolve()
        original = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")).resolve()
        if not (original / "bin/rustup").is_file():
            raise ValueError("Install Rust with rustup and update the stable toolchain first")
        cargo = build / "cargo"
        cargo.mkdir(exist_ok=True)
        # Share downloads, but never inherit the host's Cargo linker config.
        for name in ("bin", "registry", "git"):
            shared, link = original / name, cargo / name
            if shared.exists() and not link.exists():
                link.symlink_to(shared, target_is_directory=True)
        rust_paths = cargo, rustup, original
    source = build / "source"
    stage_source(root, source)
    manifest = configure_manifest(template, source, flutter, pub, settings["module"],
                                  template_path.parent, rust_paths, args.jobs)
    git_config = os.environ.get("MONODEV_GIT_CONFIG")
    if git_config:
        config_path = Path(git_config).resolve()
        if not config_path.is_file():
            raise ValueError(f"Missing temporary Git credential configuration: {config_path}")
        # Only paths enter the manifest. The credential file stays outside the
        # source snapshot and /app, mounted read-only during the build.
        manifest["build-options"]["env"].update(
            GIT_CONFIG_GLOBAL=str(config_path), CARGO_NET_GIT_FETCH_WITH_CLI="true")
        manifest["build-options"]["build-args"].append(f"--filesystem={config_path.parent}:ro")
    manifest_path = build / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    if args.prepare_only:
        print(f"Prepared {manifest_path}")
        return
    command = ["flatpak-builder"] if builder == "native" else ["flatpak", "run", "org.flatpak.Builder"]
    run(*command, "--force-clean", f"--state-dir={build / 'state'}",
        f"--repo={build / 'repo'}", str(build / "app"), str(manifest_path), cwd=root)
    arch = run("flatpak", "--default-arch", stdout=subprocess.PIPE).stdout.decode().strip()
    bundle = build / f"{profile['name']}-{arch}.flatpak"
    run("flatpak", "build-bundle", "--runtime-repo=https://flathub.org/repo/flathub.flatpakrepo",
        str(build / "repo"), str(bundle), manifest["app-id"], manifest.get("branch", "master"))
    print(f"Built {bundle}\nInstall: {shlex.join(['flatpak', 'install', '--user', str(bundle)])}")



def package_windows(root: Path, profile: dict[str, Any], bundle: Path, crt: Path) -> Path:
    binary = profile["windows"]["binary"]
    for name in (binary, "flutter_windows.dll", "data/icudtl.dat", "data/app.so"):
        if not (bundle / name).is_file():
            raise ValueError(f"Incomplete Windows release bundle: missing {name}")
    destination = root / "build/release" / artifact_name(root, profile, "windows")
    destination.parent.mkdir(parents=True, exist_ok=True)
    files = {str(path.relative_to(bundle)).replace("\\", "/"): path
             for path in bundle.rglob("*") if path.is_file() and path.suffix.lower() not in (".pdb", ".lib", ".exp")}
    files.update({path.name: path for path in crt.glob("*.dll")})
    for name in ("msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"):
        if name not in files:
            raise ValueError(f"Incomplete Windows C++ runtime: missing {name}")
    for name in profile.get("licenses", ["LICENSE"]):
        path = root / name
        if not path.is_file() or not path.resolve().is_relative_to(root.resolve()):
            raise ValueError(f"Missing license file: {path}")
        files[f"licenses/{Path(name).as_posix()}"] = path
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, path in sorted(files.items()):
            archive.write(path, name)
    return destination


def build_windows(root: Path, profile: dict[str, Any]) -> None:
    if sys.platform != "win32":
        raise ValueError("Windows releases must be built on Windows with Visual Studio C++ tools")
    if not shutil.which("flutter"):
        raise ValueError("Put Flutter on PATH before running just windows-release")
    # Windows resolves Flutter through its .bat launcher; cmd is required by the
    # supported Windows SDK distribution, rather than a cross-compilation shim.
    run("cmd", "/d", "/c", "flutter", "pub", "get", "--enforce-lockfile", cwd=root)
    run("cmd", "/d", "/c", "flutter", "build", "windows", "--release", "--no-pub", cwd=root)
    bundle = root / "build/windows/x64/runner/Release"
    print(f"Built {package_windows(root, profile, bundle, find_windows_crt())}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", default="packaging/release.json")
    commands = parser.add_subparsers(dest="command", required=True)
    flatpak = commands.add_parser("flatpak", help="Build from source inside the Flatpak SDK")
    flatpak.add_argument("--flutter-sdk", type=Path)
    flatpak.add_argument("--builder", choices=("auto", "native", "app"), default="auto")
    flatpak.add_argument("--jobs", type=int)
    flatpak.add_argument("--prepare-only", action="store_true")
    commands.add_parser("windows", help="Build and zip a Windows x64 release with its C++ runtime")
    commands.add_parser("sdk-refs", help="Print the Flatpak runtime/SDK refs required by the app manifest")
    args = parser.parse_args(argv)
    try:
        profile = load_profile(ROOT, args.profile)
        if args.command == "sdk-refs":
            template = json.loads((ROOT / profile["flatpak"]["manifest"]).read_text(encoding="utf-8"))
            branch = template["runtime-version"]
            for ref in [template["runtime"], template["sdk"], *template.get("sdk-extensions", [])]:
                print(f"{ref}//{branch}")
        elif args.command == "flatpak":
            if args.jobs is not None and args.jobs < 1:
                raise ValueError("--jobs must be positive")
            build_flatpak(ROOT, profile, args)
        else:
            build_windows(ROOT, profile)
        return 0
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        print(f"Flutter packaging failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
