#!/usr/bin/env python3
"""Contract tests for Wirt's product-neutral dependency boundary."""
from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import wirt_boundary

REPO_ROOT = Path(__file__).resolve().parents[1]


class TestWirtBoundary(unittest.TestCase):
    def test_real_workspace_has_a_clean_wirt_boundary(self):
        self.assertEqual(wirt_boundary.violations(REPO_ROOT), [])

    def test_product_dependency_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "wirt"
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n'
                '[dependencies]\narclain_core = { path = "../core" }\n',
                encoding="utf-8",
            )
            (crate / "src" / "lib.rs").write_text(
                "pub struct Neutral;\n", encoding="utf-8"
            )

            self.assertEqual(
                wirt_boundary.violations(root),
                ["crates/wirt/Cargo.toml: forbidden dependency arclain_core"],
            )

    def test_renamed_product_dependency_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "wirt"
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n'
                '[dependencies]\n'
                'neutral = { package = "arclain_core", path = "../core" }\n',
                encoding="utf-8",
            )
            (crate / "src" / "lib.rs").write_text(
                "use neutral::Service;\n", encoding="utf-8"
            )

            self.assertEqual(
                wirt_boundary.violations(root),
                ["crates/wirt/Cargo.toml: forbidden dependency arclain_core"],
            )

    def test_product_source_import_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "wirt"
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n',
                encoding="utf-8",
            )
            (crate / "src" / "lib.rs").write_text(
                "use gameta_lib::Client;\n", encoding="utf-8"
            )

            self.assertEqual(
                wirt_boundary.violations(root),
                ["crates/wirt/src/lib.rs:1: forbidden import gameta_lib"],
            )

    def test_just_check_wirt_executes_the_boundary_guard(self):
        result = subprocess.run(
            ["just", "check", "wirt"],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
        )

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
