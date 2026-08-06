#!/usr/bin/env python3
"""Assert that a lean build of `arclain_app` compiles no gameta crate.

`cargo check` and every test suite stay green when a dependency edge
stops passing `default-features = false`: feature unification silently
restores the gameta crates to the compile graph without breaking a
single assertion. Only the resolved dependency tree shows it, so this
guard reads the tree itself.

The defaults tree is a positive control. A renamed crate or a broken
match would otherwise let the lean assertion pass vacuously.

After the tree assertion it runs the lean check and the two lean test
suites. Those exercise the `cfg(not(feature = "gameta"))` fallback
behavior, which no defaults-workspace invocation ever compiles.
"""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
NEEDLE = "gameta"
TREE_GLYPHS = "│├└─ \t"
DEFAULTS_TREE = ["cargo", "tree", "-p", "arclain_app"]
LEAN_TREE = ["cargo", "tree", "-p", "arclain_app", "--no-default-features"]
LEAN_COMMANDS = (
    ["cargo", "check", "-p", "arclain_app", "--no-default-features", "--all-targets"],
    ["cargo", "test", "-p", "arclain_core", "--no-default-features"],
    ["cargo", "test", "-p", "arclain_app", "--no-default-features"],
)


def gameta_lines(command: list[str]) -> list[str] | None:
    """Return the tree lines naming a gameta crate, or None if cargo failed.

    Each line keeps only its crate part: cargo prefixes the tree with box
    characters, and the console this may print to is not always able to
    encode them (see `main`).
    """
    result = subprocess.run(
        command,
        cwd=REPO_ROOT,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        print(f"check-gameta: `{' '.join(command)}` failed")
        return None
    return [
        line.lstrip(TREE_GLYPHS)
        for line in result.stdout.splitlines()
        if NEEDLE in line.lower()
    ]


def assert_trees() -> int:
    """Compare the defaults and lean dependency trees of `arclain_app`."""
    defaults = gameta_lines(DEFAULTS_TREE)
    if defaults is None:
        return 1
    if not defaults:
        print("check-gameta control failed: defaults tree shows no gameta")
        return 1

    lean = gameta_lines(LEAN_TREE)
    if lean is None:
        return 1
    if lean:
        print("check-gameta FAILED: gameta in the no-default-features tree:")
        for line in lean:
            print(line)
        return 1

    print(f"check-gameta OK (defaults tree: {len(defaults)} gameta lines, lean tree: 0)")
    return 0


def main() -> int:
    # A console in a non-UTF-8 code page cannot encode everything cargo
    # may put in a path or an error, and a guard must report its failure
    # rather than die encoding it. Line buffering keeps the verdict in
    # order with the cargo output that follows it when both are piped.
    sys.stdout.reconfigure(errors="replace", line_buffering=True)
    sys.stderr.reconfigure(errors="replace", line_buffering=True)

    status = assert_trees()
    if status != 0:
        return status
    for command in LEAN_COMMANDS:
        result = subprocess.run(command, cwd=REPO_ROOT)
        if result.returncode != 0:
            return result.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
