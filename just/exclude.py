#!/usr/bin/env python3
"""Keep noise out of language servers and search tools.

    python just/exclude.py worktrees [--scope dart|ts|rust|godot|editor|all]

A git worktree is a full copy of the repo. gitignore does not stop an analyzer:
each copy is discovered as another project, so the repo is indexed once per
checkout and every finding is reported that many times. This writes the
exclusion into the file each tool actually reads.

Vendored from monodev (canonical: monodev/just/exclude.py) — do not edit here;
change it in monodev and re-run `just -g deploy`. Stdlib only.
"""
from __future__ import annotations

import argparse
import copy
import json
import sys
from pathlib import Path

# preset -> (glob entries for analyzers, bare dir names, watcher globs)
PRESETS = {
    "worktrees": {
        "globs": [".worktrees/**", ".claude/**"],
        "dirs": [".worktrees", ".claude"],
        "watch": ["**/.worktrees/**", "**/.claude/worktrees/**"],
    },
}

BEGIN = "# >>> just exclude: {preset} (managed) >>>"
END = "# <<< just exclude: {preset} (managed) <<<"

IGNORE_HEADER = (
    "# Managed by `just exclude` (monodev). Re-run it instead of editing.\n"
    "#\n"
    "# Git worktrees are full copies of this repo; tools that read .ignore skip\n"
    "# them here so one match stops showing up once per checkout.\n"
)


# ── pure helpers ──────────────────────────────────────────────────────────

def _managed_block(preset: str, entries: list[str], indent: str) -> list[str]:
    lines = [indent + BEGIN.format(preset=preset)]
    lines += [f"{indent}- {e}" for e in entries]
    lines += [indent + END.format(preset=preset)]
    return lines


def _strip_managed(lines: list[str], preset: str) -> list[str]:
    begin, end = BEGIN.format(preset=preset), END.format(preset=preset)
    out, skipping = [], False
    for line in lines:
        s = line.strip()
        if s == begin:
            skipping = True
            continue
        if s == end:
            skipping = False
            continue
        if not skipping:
            out.append(line)
    return out


def analysis_options_with_excludes(text: str, entries: list[str], preset: str = "worktrees") -> str:
    """Insert the managed exclude block into a Dart analysis_options.yaml.

    Hand-written excludes, comments and later sections are preserved: the block is
    delimited by markers and only that region is rewritten.
    """
    lines = _strip_managed(text.splitlines(), preset)

    # Anything already excluded by hand stays that way — never restate it.
    present = {l.strip()[2:].strip() for l in lines if l.strip().startswith("- ")}
    entries = [e for e in entries if e not in present]
    if not entries:
        return "\n".join(lines).rstrip("\n") + "\n" if lines else ""

    analyzer_at = next((i for i, l in enumerate(lines) if l.rstrip() == "analyzer:"), None)
    if analyzer_at is None:
        block = ["analyzer:", "  exclude:"] + _managed_block(preset, entries, "    ")
        body = lines + ([""] if lines and lines[-1].strip() else []) + block
        return "\n".join(body).rstrip("\n") + "\n"

    # bounds of the analyzer: mapping (until the next top-level key)
    end_at = len(lines)
    for i in range(analyzer_at + 1, len(lines)):
        l = lines[i]
        if l.strip() and not l[0].isspace():
            end_at = i
            break

    exclude_at = next(
        (i for i in range(analyzer_at + 1, end_at) if lines[i].strip() == "exclude:"), None
    )
    if exclude_at is None:
        insert = ["  exclude:"] + _managed_block(preset, entries, "    ")
        lines[analyzer_at + 1:analyzer_at + 1] = insert
        return "\n".join(lines).rstrip("\n") + "\n"

    # match the indentation the existing list items use
    indent = "    "
    for i in range(exclude_at + 1, end_at):
        s = lines[i]
        if s.strip().startswith("- "):
            indent = s[: len(s) - len(s.lstrip())]
            break
    # append after the existing items of that list
    last = exclude_at
    for i in range(exclude_at + 1, end_at):
        if lines[i].strip().startswith("- ") or not lines[i].strip():
            last = i if lines[i].strip() else last
        else:
            break
    lines[last + 1:last + 1] = _managed_block(preset, entries, indent)
    return "\n".join(lines).rstrip("\n") + "\n"


def tsconfig_needs_excludes(obj: dict) -> bool:
    """Whether this tsconfig could pick up a worktree in the first place.

    An `include` listing rooted paths (`src/**/*.ts`) can never match `.worktrees/`,
    so adding `exclude` there changes nothing and only churns the file. Absent or
    wildcard-leading include patterns do reach everything.
    """
    include = obj.get("include")
    if not include:
        return True
    return any(p.startswith("*") or p.startswith(".") or p.startswith("/") for p in include)


def tsconfig_with_excludes(obj: dict, entries: list[str]) -> dict:
    out = copy.deepcopy(obj)
    current = list(out.get("exclude", []))
    for e in entries:
        if e not in current:
            current.append(e)
    out["exclude"] = current
    return out


def editor_settings_with_excludes(obj: dict, preset: str = "worktrees") -> dict:
    p = PRESETS[preset]
    out = copy.deepcopy(obj)
    for key in ("rust-analyzer.files.excludeDirs", "dart.analysisExcludedFolders"):
        current = list(out.get(key, []))
        for d in p["dirs"]:
            if d not in current:
                current.append(d)
        out[key] = current
    for key in ("files.watcherExclude", "search.exclude"):
        block = dict(out.get(key, {}))
        for g in p["watch"]:
            block.setdefault(g, True)
        out[key] = block
    return out


def ignore_file_content(entries: list[str]) -> str:
    return IGNORE_HEADER + "".join(f"{e.removesuffix('/**')}/\n" for e in entries)


SKIP_DIRS = {"build", "node_modules", "target", "dist", "vendor", "third_party"}


def _has_marker(repo: Path, marker: str) -> bool:
    """True if `marker` sits at the repo root or in an immediate subdirectory.

    A package one level down still counts — transmute keeps its Flutter app in
    flutter/. Hidden directories are skipped so a pubspec inside a worktree copy
    (.worktrees/branch/pubspec.yaml) does not make every repo look like a Dart
    project, and the scan stops at one level so it stays cheap and predictable.
    """
    if (repo / marker).exists():
        return True
    for child in repo.iterdir():
        if not child.is_dir() or child.name.startswith(".") or child.name in SKIP_DIRS:
            continue
        if (child / marker).exists():
            return True
    return False


def detect_toolchains(repo: Path) -> list[str]:
    repo = Path(repo)
    if not repo.is_dir():
        return []
    found = []
    for marker, name in (
        ("pubspec.yaml", "dart"),
        ("tsconfig.json", "ts"),
        ("Cargo.toml", "rust"),
        ("project.godot", "godot"),
    ):
        if _has_marker(repo, marker):
            found.append(name)
    return found


# ── apply ─────────────────────────────────────────────────────────────────

def apply(repo: Path, preset: str = "worktrees", scope: str = "auto") -> list[str]:
    """Write the exclusions this repo needs. Returns human-readable changes."""
    if preset not in PRESETS:
        raise SystemExit(f"unknown preset '{preset}' (have: {', '.join(sorted(PRESETS))})")
    repo = Path(repo)
    p = PRESETS[preset]
    changed: list[str] = []
    want = detect_toolchains(repo) if scope == "auto" else [scope]

    def write(path: Path, text: str) -> None:
        if path.exists() and path.read_text(encoding="utf-8") == text:
            return
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8", newline="\n")
        changed.append(str(path.relative_to(repo)))

    if scope in ("auto", "all", "search"):
        write(repo / ".ignore", ignore_file_content(p["globs"]))

    if "dart" in want:
        f = repo / "analysis_options.yaml"
        src = f.read_text(encoding="utf-8") if f.exists() else ""
        write(f, analysis_options_with_excludes(src, p["globs"], preset))

    if "ts" in want:
        f = repo / "tsconfig.json"
        if f.exists():
            raw = f.read_text(encoding="utf-8")
            try:
                obj = json.loads(raw)
            except json.JSONDecodeError:
                changed.append("SKIPPED (not strict JSON, edit by hand): tsconfig.json")
            else:
                # Rewriting reflows the whole file, so only do it when it helps.
                if tsconfig_needs_excludes(obj):
                    write(f, json.dumps(tsconfig_with_excludes(obj, p["globs"]), indent=2) + "\n")

    if "godot" in want:
        # Godot skips any directory containing a .gdignore file.
        for d in p["dirs"]:
            if (repo / d).is_dir():
                write(repo / d / ".gdignore", "")

    if scope in ("auto", "all", "editor"):
        f = repo / ".vscode" / "settings.json"
        if f.exists():
            raw = f.read_text(encoding="utf-8")
            try:
                obj = json.loads(raw)
            except json.JSONDecodeError:
                changed.append("SKIPPED (not strict JSON, edit by hand): .vscode/settings.json")
            else:
                write(f, json.dumps(editor_settings_with_excludes(obj, preset), indent=2) + "\n")

    return changed


def main(argv=None) -> int:
    try:
        sys.stdout.reconfigure(errors="replace")
    except (AttributeError, ValueError):
        pass
    ap = argparse.ArgumentParser(prog="exclude.py")
    ap.add_argument("preset", nargs="?", default="worktrees", choices=sorted(PRESETS))
    ap.add_argument("--scope", default="auto",
                    help="auto (detect) | dart | ts | rust | godot | editor | search | all")
    ap.add_argument("--repo", default=".")
    a = ap.parse_args(argv)
    repo = Path(a.repo).resolve()
    changes = apply(repo, a.preset, a.scope)
    for c in changes:
        print(f"  {c}")
    print(f"  {repo.name}: up to date" if not changes else f"  {repo.name}: {len(changes)} file(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
