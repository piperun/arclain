#!/usr/bin/env python3
"""Run `cargo ui` with RUST_LOG built from scripts/logging_config.json.

Usage:
    python scripts/_ui.py                          # cargo ui
    python scripts/_ui.py --features dev-foo       # cargo ui --features dev-foo

Extra arguments are forwarded verbatim to `cargo ui`.

`logging_config.json` shape:
    { "default_level": "info",
      "filters": { "arclain": "debug", "wgpu": "warn" } }
becomes RUST_LOG="info,arclain=debug,wgpu=warn". Falls back to a
sensible default when the JSON is missing or malformed.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
LOGGING_CONFIG = SCRIPT_DIR / "logging_config.json"
DEFAULT_RUST_LOG = "arclain=debug,info"


def load_rust_log() -> str:
    if not LOGGING_CONFIG.exists():
        print(f"  {LOGGING_CONFIG.name} not found, using default")
        return DEFAULT_RUST_LOG
    try:
        data = json.loads(LOGGING_CONFIG.read_text())
    except (OSError, json.JSONDecodeError) as e:
        print(f"  Failed to parse {LOGGING_CONFIG.name}: {e}")
        return DEFAULT_RUST_LOG

    parts = [data.get("default_level", "info")]
    for module, level in data.get("filters", {}).items():
        parts.append(f"{module}={level}")
    return ",".join(parts)


def main() -> None:
    rust_log = load_rust_log()
    print(f"RUST_LOG = {rust_log}")

    env = {**os.environ, "RUST_LOG": rust_log, "CARGO_TERM_COLOR": "always"}
    cmd = ["cargo", "ui", *sys.argv[1:]]
    print(f"  > {' '.join(cmd)}")
    result = subprocess.run(cmd, cwd=REPO_ROOT, env=env)
    sys.exit(result.returncode)


if __name__ == "__main__":
    main()
