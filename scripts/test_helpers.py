#!/usr/bin/env python3
"""Unit + smoke tests for the arclain build helpers.

Run:
    python -m unittest discover -s scripts -p "test_*.py"
    (or:  just test-scripts)

Covers the platform-name mapping, RUST_LOG assembly from JSON, workspace
version parsing, that every helper imports cleanly, and that the justfile
parses.
"""
from __future__ import annotations

import importlib
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import _package
import _ui
from _package import get_platform, workspace_version
from _ui import load_rust_log

REPO_ROOT = Path(__file__).resolve().parents[1]


class TestGetPlatform(unittest.TestCase):
    def _run(self, system: str, machine: str) -> tuple[str, str]:
        with mock.patch.object(_package.platform, "system", return_value=system), \
             mock.patch.object(_package.platform, "machine", return_value=machine):
            return get_platform()

    def test_windows_x64(self):
        self.assertEqual(self._run("Windows", "AMD64"), ("windows", "x64"))

    def test_linux_x64(self):
        self.assertEqual(self._run("Linux", "x86_64"), ("linux", "x64"))

    def test_macos_arm64(self):
        self.assertEqual(self._run("Darwin", "arm64"), ("macos", "arm64"))

    def test_linux_aarch64(self):
        self.assertEqual(self._run("Linux", "aarch64"), ("linux", "arm64"))

    def test_unknown_machine_passthrough(self):
        self.assertEqual(self._run("Linux", "riscv64"), ("linux", "riscv64"))


class TestLoadRustLog(unittest.TestCase):
    def test_assembles_from_json(self):
        with tempfile.TemporaryDirectory() as d:
            cfg = Path(d) / "logging_config.json"
            cfg.write_text(json.dumps({
                "default_level": "info",
                "filters": {"arclain": "debug", "wgpu": "warn"},
            }))
            with mock.patch.object(_ui, "LOGGING_CONFIG", cfg):
                self.assertEqual(load_rust_log(), "info,arclain=debug,wgpu=warn")

    def test_missing_file_returns_default(self):
        with mock.patch.object(_ui, "LOGGING_CONFIG", Path("/no/such/file.json")):
            self.assertEqual(load_rust_log(), _ui.DEFAULT_RUST_LOG)

    def test_malformed_json_returns_default(self):
        with tempfile.TemporaryDirectory() as d:
            cfg = Path(d) / "logging_config.json"
            cfg.write_text("{ not valid json")
            with mock.patch.object(_ui, "LOGGING_CONFIG", cfg):
                self.assertEqual(load_rust_log(), _ui.DEFAULT_RUST_LOG)


class TestWorkspaceVersion(unittest.TestCase):
    def test_reads_real_cargo_toml(self):
        # Parses [workspace.package].version from the real root Cargo.toml.
        self.assertRegex(workspace_version(), r"^\d+\.\d+\.\d+")


class TestSmoke(unittest.TestCase):
    def test_helpers_import_cleanly(self):
        for name in ("_plugins", "_deps", "_package", "_ui"):
            assert importlib.import_module(name) is not None

    @unittest.skipUnless(shutil.which("just"), "just not on PATH")
    def test_justfile_parses(self):
        result = subprocess.run(
            ["just", "--list"], cwd=REPO_ROOT, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        for recipe in ("debug", "release", "plugins", "ui"):
            self.assertIn(recipe, result.stdout)


if __name__ == "__main__":
    unittest.main()
