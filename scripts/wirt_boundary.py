#!/usr/bin/env python3
"""Check that Wirt remains independent of product-specific code."""
from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

FORBIDDEN_PREFIXES = ("arclain_", "gameta_", "filer")
FORBIDDEN_EXACT = {
    "egui", "eframe", "flutter_rust_bridge", "frb",
    "dart_api", "allo_isolate",
}
IMPORT_LINE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:use|extern\s+crate)\b"
)
IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def normalized(name: str) -> str:
    return name.replace("-", "_").lower()


def forbidden(name: str) -> bool:
    value = normalized(name)
    return value in FORBIDDEN_EXACT or value.startswith(FORBIDDEN_PREFIXES)


def dependency_tables(document: dict) -> list[dict]:
    tables = [
        document.get(name, {})
        for name in ("dependencies", "build-dependencies", "dev-dependencies")
    ]
    for target in document.get("target", {}).values():
        tables.extend(
            target.get(name, {})
            for name in ("dependencies", "build-dependencies", "dev-dependencies")
        )
    return tables


def dependency_violations(workspace_root: Path) -> list[str]:
    manifest = workspace_root / "crates" / "wirt" / "Cargo.toml"
    if not manifest.exists():
        return ["crates/wirt/Cargo.toml: missing neutral crate manifest"]
    with manifest.open("rb") as handle:
        document = tomllib.load(handle)
    names = []
    for table in dependency_tables(document):
        for name, dependency in table.items():
            package = (
                dependency.get("package", name)
                if isinstance(dependency, dict)
                else name
            )
            if forbidden(package):
                names.append(package)
    names.sort()
    return [f"crates/wirt/Cargo.toml: forbidden dependency {name}" for name in names]


def source_violations(workspace_root: Path) -> list[str]:
    source_root = workspace_root / "crates" / "wirt" / "src"
    violations: list[str] = []
    for path in sorted(source_root.rglob("*.rs")) if source_root.exists() else []:
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if not IMPORT_LINE.match(line):
                continue
            for name in IDENTIFIER.findall(line):
                if forbidden(name):
                    relative = path.relative_to(workspace_root).as_posix()
                    violations.append(f"{relative}:{number}: forbidden import {name}")
                    break
    return violations


def violations(workspace_root: Path) -> list[str]:
    return dependency_violations(workspace_root) + source_violations(workspace_root)


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    found = violations(root)
    for violation in found:
        print(violation)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
