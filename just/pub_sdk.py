"""Select installed SDKs and prepare the locked, application-independent YAML reader."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
from typing import Any

READER_FILES = ("pubspec.yaml", "pubspec.lock", "bin/read.dart", "lib/reader.dart")
READER_SOURCE = Path(__file__).resolve().parent / "pub_reader"


def flutter_root(root: Path) -> Path:
    selected = os.environ.get("FLUTTER_ROOT")
    local = root / ".fvm/flutter_sdk"
    if not selected and local.exists():
        selected = str(local)
    if not selected:
        launcher = shutil.which("flutter")
        if launcher:
            selected = str(Path(launcher).resolve().parent.parent)
    if not selected:
        raise ValueError("Put an installed Flutter SDK on PATH or set FLUTTER_ROOT")
    sdk = Path(selected).resolve()
    if not (sdk / "packages/flutter_tools").is_dir():
        raise ValueError(f"Not a Flutter SDK: {sdk}")
    return sdk


def dart_executable(root: Path, kind: str = "dart") -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    if kind == "flutter":
        path = flutter_root(root) / f"bin/cache/dart-sdk/bin/dart{suffix}"
    elif os.environ.get("DART_SDK"):
        path = Path(os.environ["DART_SDK"]) / f"bin/dart{suffix}"
    elif (root / ".fvm/flutter_sdk").exists():
        path = flutter_root(root) / f"bin/cache/dart-sdk/bin/dart{suffix}"
    else:
        executable = shutil.which("dart")
        if not executable:
            raise ValueError("Put an installed Dart or Flutter SDK on PATH")
        path = Path(executable).resolve()
        bundled = path.parent / f"cache/dart-sdk/bin/dart{suffix}"
        if bundled.is_file():
            path = bundled
    if not path.is_file():
        raise ValueError(f"SDK is not prepared; install its Dart SDK first: {path}")
    return path


def dart_version(executable: Path) -> str:
    result = subprocess.run([str(executable), "--version"], check=True, capture_output=True, text=True)
    match = re.search(r"Dart SDK version: (\d+\.\d+\.\d+) \(stable\)", result.stdout + result.stderr)
    if not match:
        raise ValueError("Pub tooling requires an installed stable Dart SDK with an exact version")
    return match[1]


def flutter_version(root: Path) -> str:
    sdk = flutter_root(root)
    metadata = json.loads((sdk / "bin/cache/flutter.version.json").read_text(encoding="utf-8"))
    if (sdk / ".git").exists():
        sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=sdk, check=True,
                             capture_output=True, text=True).stdout.strip()
        if metadata.get("frameworkRevision") != sha:
            raise ValueError("Flutter SDK cached version does not match its checkout")
    value = metadata.get("frameworkVersion")
    if not isinstance(value, str) or not re.fullmatch(r"\d+\.\d+\.\d+", value):
        raise ValueError("Flutter SDK needs exact stable cached version metadata")
    return value


def select(root: Path, saved: dict[str, Any] | None = None, *, need_flutter: bool = False,
           dart_pin: str | None = None, flutter_pin: str | None = None) -> dict[str, str]:
    saved = saved or {}
    if need_flutter or flutter_pin or saved.get("kind") == "flutter":
        pins: set[str] = set()
        if flutter_pin:
            pins.add(flutter_pin)
        else:
            for relative, key in ((".fvmrc", "flutter"), (".fvm/fvm_config.json", "flutterSdkVersion"),
                                  (".flutter-version", None)):
                path = root / relative
                if path.is_file():
                    value = json.loads(path.read_text()).get(key) if key else path.read_text().strip()
                    if not isinstance(value, str):
                        raise ValueError(f"Invalid SDK pin in {relative}")
                    pins.add(value)
            if not pins and saved:
                pins.add(saved["version"])
        installed = flutter_version(root)
        expected = next(iter(pins)) if len(pins) == 1 else installed if not pins else ""
        kind = "flutter"
    else:
        path = root / ".dart-version"
        expected = dart_pin or (path.read_text().strip() if path.is_file() else saved.get("version"))
        installed = dart_version(dart_executable(root))
        expected = expected or installed
        kind = "dart"
    if not re.fullmatch(r"\d+\.\d+\.\d+", expected) or expected != installed:
        raise ValueError(f"Use the pinned {kind} SDK {expected!r}; installed version is {installed}")
    return {"kind": kind, "version": expected}


def validate(root: Path, sdk: dict[str, str]) -> Path:
    if set(sdk) != {"kind", "version"} or sdk["kind"] not in ("dart", "flutter"):
        raise ValueError("Pub sdk needs kind dart/flutter and exact version")
    selected = select(root, sdk, dart_pin=sdk["version"] if sdk["kind"] == "dart" else None,
                      flutter_pin=sdk["version"] if sdk["kind"] == "flutter" else None)
    return dart_executable(root, selected["kind"])


def native_command(root: Path, sdk: dict[str, str], *args: str) -> list[str]:
    executable = validate(root, sdk)
    if sdk["kind"] == "dart":
        return [str(executable), "pub", *args]
    launcher = flutter_root(root) / ("bin/flutter.bat" if os.name == "nt" else "bin/flutter")
    if os.name == "nt":
        if any(c in str(launcher) for c in '%!"\r\n'):
            raise ValueError("Flutter SDK path contains unsupported Windows command characters")
        # Always quote the batch path, even when it contains & without spaces.
        command = '""' + str(launcher) + '" ' + subprocess.list2cmdline(["pub", *args]) + '"'
        return [os.environ.get("COMSPEC", "cmd.exe"), "/d", "/v:off", "/s", "/c", command]
    return [str(launcher), "pub", *args]


def reader_request(executable: Path, payload: dict[str, Any]) -> Any:
    version = dart_version(executable)
    sources = {relative: (READER_SOURCE / relative).read_bytes() for relative in READER_FILES}
    identity = hashlib.sha256(version.encode() + b"".join(k.encode() + v for k, v in sources.items())).hexdigest()
    base = os.environ.get("MONODEV_TOOL_CACHE")
    if not base:
        base = str(Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData/Local")) if os.name == "nt"
                   else Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")))
        base = str(Path(base) / "monodev/pub-reader")
    cache = Path(base) / identity
    # Do not follow cache symlinks into another application's files.
    current = cache.absolute()
    for part in (current, *current.parents):
        if part.is_symlink():
            raise ValueError(f"Symlinked tooling cache: {part}")
    cache.mkdir(parents=True, exist_ok=True)
    for relative in (*READER_FILES, ".prepared", ".dart_tool/package_config.json"):
        current = cache
        for part in Path(relative).parts:
            current /= part
            if current.is_symlink():
                raise ValueError(f"Symlinked tooling cache: {current}")
    for relative, content in sources.items():
        path = cache / relative
        if path.exists() and (path.is_symlink() or path.read_bytes() != content):
            raise ValueError(f"Tooling cache content differs; remove this cache entry: {cache}")
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
    marker = cache / ".prepared"
    if not marker.is_file() or not (cache / ".dart_tool/package_config.json").is_file():
        print(f"Preparing locked Pub metadata tooling: {cache}", file=sys.stderr)
        env = dict(os.environ, PUB_HOSTED_URL="https://pub.dev")
        result = subprocess.run([str(executable), "pub", "get", "--offline", "--enforce-lockfile"],
                                cwd=cache, env=env, capture_output=True, text=True)
        if result.returncode:
            result = subprocess.run([str(executable), "pub", "get", "--enforce-lockfile"],
                                    cwd=cache, env=env, capture_output=True, text=True)
        if result.returncode:
            raise ValueError(f"Cannot prepare Pub metadata tooling: {result.stderr or result.stdout}")
        if (cache / "pubspec.lock").read_bytes() != sources["pubspec.lock"]:
            raise ValueError("Pub tooling unexpectedly changed its locked dependencies")
        marker.write_text(version + "\n")
    result = subprocess.run([str(executable), "--packages=" + str(cache / ".dart_tool/package_config.json"),
                             str(cache / "bin/read.dart")], input=json.dumps(payload),
                            capture_output=True, text=True, check=True, cwd=cache)
    return json.loads(result.stdout)
