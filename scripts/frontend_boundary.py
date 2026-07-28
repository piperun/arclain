#!/usr/bin/env python3
"""Frontend/headless dependency boundary guard.

Arclain's crates split into two categories:

- HEADLESS_CRATES hold business logic, storage, networking and plugin
  hosting. They must stay usable without any GUI toolkit -- no egui,
  eframe, Flutter bridge, or Dart FFI code, and no dependency on a GUI
  crate.
- GUI_CRATES (theme, ui, widgets) are frontends. They may depend on
  headless crates in principle, but Stage 1's target architecture routes
  the frontend through a single facade crate (`app`, package
  `arclain_app`) instead of reaching into headless internals directly, so
  every direct dependency on a headless crate *other than* `app` is
  migration-baseline work still to do -- see SANCTIONED_FRONTEND_DEPENDENCY.

This module exposes two static checks:

- `dependency_violations` inspects Cargo.toml path dependencies (normal,
  build, dev, and target-specific tables) for edges that cross the
  headless/GUI boundary in either direction.
- `source_violations` scans headless crates' `src/` trees for forbidden
  GUI-toolkit and bridge references.

Run:
    python scripts/frontend_boundary.py
    (or:  just frontend-boundary)

Prints one violation per line and exits 1 if any were found, 0 otherwise.
"""
from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path
from typing import Iterator

REPO_ROOT = Path(__file__).resolve().parents[1]

HEADLESS_CRATES = {
    "app", "app_fs", "checksum", "core", "data", "db",
    "network", "plugins", "signals",
}
GUI_CRATES = {"theme", "ui", "widgets"}

# `app` (`arclain_app`) is the Stage 1 application facade: the one
# headless crate a GUI crate is *meant* to depend on, replacing direct
# dependencies on every other headless crate over time. A GUI crate
# depending on it is therefore accepted rather than flagged as
# migration-baseline debt; a GUI crate depending on any other headless
# crate remains a violation.
SANCTIONED_FRONTEND_DEPENDENCY = "app"

# Dependency tables to inspect, per manifest and per `[target.'cfg(...)'.*]`
# section.
_DEPENDENCY_TABLE_NAMES = ("dependencies", "build-dependencies", "dev-dependencies")

# Flutter bridge / Dart FFI crate identifiers. These are matched only on
# use/extern-crate statement lines, case-insensitively: that is how they
# would realistically appear in code, and restricting the match to import
# statements avoids tripping on incidental prose.
_BRIDGE_IDENTIFIERS = ("flutter_rust_bridge", "frb", "dart_api", "allo_isolate")

# egui/eframe are matched anywhere in a non-comment line (not just
# use/extern lines): headless code can reference `egui::Context` etc. via a
# fully-qualified path with no `use` statement at all, and that is exactly
# the kind of silent coupling this guard exists to catch. Comment lines are
# excluded (see _COMMENT_LINE below) since doc-comment prose and fenced
# ```ignore examples routinely mention these names without compiling.
_GUI_TOOLKIT_IDENTIFIERS = ("egui", "eframe")

_USE_STATEMENT = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:use|extern\s+crate)\b")

# Rust line comments (`//`, `///`, `//!`). Doc-comment prose and fenced
# ```ignore examples routinely mention egui/eframe to describe an
# integration point or a hypothetical caller -- that is not compiled code
# and must not be flagged, and must not let a doc comment "win" the
# first-hit line over the real usage site it documents below it.
_COMMENT_LINE = re.compile(r"^\s*//")


def _identifier_pattern(name: str, *, case_insensitive: bool) -> re.Pattern[str]:
    flags = re.IGNORECASE if case_insensitive else 0
    return re.compile(rf"\b{re.escape(name)}\b", flags)


def _iter_crate_manifests(
    workspace_root: Path, names: set[str],
) -> Iterator[tuple[str, Path, Path]]:
    """Yield (crate_name, crate_dir, manifest_path) for each crate under
    workspace_root/crates whose directory name is in `names` and that has a
    Cargo.toml. Crates listed in `names` but not present on disk (e.g. `app`
    before it exists) are silently skipped."""
    crates_dir = workspace_root / "crates"
    if not crates_dir.is_dir():
        return
    for crate_dir in sorted(p for p in crates_dir.iterdir() if p.is_dir()):
        name = crate_dir.name
        if name not in names:
            continue
        manifest_path = crate_dir / "Cargo.toml"
        if manifest_path.is_file():
            yield name, crate_dir, manifest_path


def _load_manifest(manifest_path: Path) -> dict:
    with manifest_path.open("rb") as handle:
        return tomllib.load(handle)


def _dependency_tables(manifest: dict) -> Iterator[tuple[str, dict]]:
    """Yield (table_label, table) for every normal/build/dev dependency
    table in a manifest, including the target-specific variants under
    `[target.'cfg(...)'.*]`."""
    for table_name in _DEPENDENCY_TABLE_NAMES:
        table = manifest.get(table_name)
        if isinstance(table, dict):
            yield table_name, table
    target = manifest.get("target")
    if isinstance(target, dict):
        for cfg_key, cfg_table in target.items():
            if not isinstance(cfg_table, dict):
                continue
            for table_name in _DEPENDENCY_TABLE_NAMES:
                table = cfg_table.get(table_name)
                if isinstance(table, dict):
                    yield f"target.{cfg_key}.{table_name}", table


def dependency_violations(workspace_root: Path) -> list[str]:
    """Report every workspace path-dependency that crosses the headless/GUI
    boundary, in either direction. Same-category path dependencies (headless
    depending on headless, GUI depending on GUI) are accepted."""
    violations = []
    all_names = HEADLESS_CRATES | GUI_CRATES
    for crate_name, crate_dir, manifest_path in _iter_crate_manifests(workspace_root, all_names):
        is_headless = crate_name in HEADLESS_CRATES
        forbidden = GUI_CRATES if is_headless else (HEADLESS_CRATES - {SANCTIONED_FRONTEND_DEPENDENCY})
        manifest = _load_manifest(manifest_path)
        for table_label, table in _dependency_tables(manifest):
            for dep_name, dep_value in table.items():
                if not isinstance(dep_value, dict):
                    continue
                path = dep_value.get("path")
                if not isinstance(path, str):
                    continue
                target_name = (crate_dir / path).resolve().name
                if target_name not in forbidden:
                    continue
                if is_headless:
                    reason = "headless crate must not depend on a GUI crate"
                else:
                    reason = (
                        "frontend must not depend directly on a headless "
                        "crate (migration baseline)"
                    )
                violations.append(
                    f"crates/{crate_name} [{table_label}] depends on "
                    f"crates/{target_name} via {dep_name!r}: {reason}"
                )
    return sorted(violations)


def source_violations(workspace_root: Path) -> list[str]:
    """Report forbidden GUI-toolkit and bridge references inside headless
    crates' source trees (crates/<headless>/src/**/*.rs)."""
    whole_file_patterns = {
        name: _identifier_pattern(name, case_insensitive=False)
        for name in _GUI_TOOLKIT_IDENTIFIERS
    }
    statement_only_patterns = {
        name: _identifier_pattern(name, case_insensitive=True)
        for name in _BRIDGE_IDENTIFIERS
    }
    for gui_crate in sorted(GUI_CRATES):
        crate_ident = f"arclain_{gui_crate}"
        statement_only_patterns[crate_ident] = _identifier_pattern(
            crate_ident, case_insensitive=False,
        )

    violations = []
    for crate_name, crate_dir, _manifest_path in _iter_crate_manifests(
        workspace_root, HEADLESS_CRATES,
    ):
        src_dir = crate_dir / "src"
        if not src_dir.is_dir():
            continue
        for source_path in sorted(src_dir.rglob("*.rs")):
            relative = source_path.relative_to(crate_dir).as_posix()
            lines = source_path.read_text(encoding="utf-8").splitlines()
            first_hit_line: dict[str, int] = {}
            for line_no, line in enumerate(lines, start=1):
                if not _COMMENT_LINE.match(line):
                    for name, pattern in whole_file_patterns.items():
                        if name not in first_hit_line and pattern.search(line):
                            first_hit_line[name] = line_no
                if _USE_STATEMENT.match(line):
                    for name, pattern in statement_only_patterns.items():
                        if name not in first_hit_line and pattern.search(line):
                            first_hit_line[name] = line_no
            for name, line_no in first_hit_line.items():
                violations.append(
                    f"crates/{crate_name}/{relative}:{line_no}: forbidden "
                    f"reference to {name!r} in a headless crate source tree"
                )
    return sorted(violations)


def main() -> int:
    violations = dependency_violations(REPO_ROOT) + source_violations(REPO_ROOT)
    for violation in violations:
        print(violation)
    return 1 if violations else 0


if __name__ == "__main__":
    sys.exit(main())
