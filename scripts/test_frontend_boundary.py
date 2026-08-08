#!/usr/bin/env python3
"""Unit tests for the frontend/headless dependency boundary guard.

Run:
    python scripts/test_frontend_boundary.py
    (or:  just test-frontend-boundary)

These tests build miniature throwaway cargo workspaces under a tempdir --
plain Cargo.toml / .rs text files, no real cargo invocation -- and check
`dependency_violations` / `source_violations` against them directly.
"""
from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import frontend_boundary

REPO_ROOT = Path(__file__).resolve().parents[1]


def _write_manifest(crates_dir: Path, name: str, body: str) -> Path:
    """Create crates_dir/<name>/Cargo.toml with the given text and return
    the crate directory."""
    crate_dir = crates_dir / name
    crate_dir.mkdir(parents=True, exist_ok=True)
    (crate_dir / "Cargo.toml").write_text(body, encoding="utf-8")
    return crate_dir


def _write_source_file(crate_dir: Path, relative: str, content: str) -> Path:
    """Create crate_dir/src/<relative> with the given text and return its path."""
    path = crate_dir / "src" / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    return path


class TestModuleConstants(unittest.TestCase):
    def test_headless_and_gui_crate_sets_match_the_specified_boundary(self):
        self.assertEqual(
            frontend_boundary.HEADLESS_CRATES,
            {
                "app", "app_fs", "checksum", "core", "data", "db",
                "network", "plugins", "signals", "wirt",
            },
        )
        self.assertEqual(frontend_boundary.GUI_CRATES, {"theme", "ui", "widgets"})

    def test_frontend_crates_is_gui_crates_plus_cli(self):
        # `cli` (arclain-cli) is a frontend in the same headless-dependency
        # sense as the GUI crates -- routes through `app` instead of
        # reaching into headless internals -- but embeds no GUI toolkit,
        # so it is deliberately absent from GUI_CRATES itself (which also
        # names source_violations's egui/eframe scan target).
        self.assertEqual(
            frontend_boundary.FRONTEND_CRATES,
            {"theme", "ui", "widgets", "cli"},
        )


class TestDependencyViolations(unittest.TestCase):
    def test_rejects_a_gui_dependency_from_a_headless_crate(self):
        with tempfile.TemporaryDirectory() as workspace:
            crates = Path(workspace) / "crates"
            _write_manifest(crates, "core", (
                '[package]\nname = "arclain_core"\n\n'
                '[dependencies]\n'
                'arclain_ui = { path = "../ui" }\n'
            ))
            _write_manifest(crates, "ui", '[package]\nname = "arclain_ui"\n')

            violations = frontend_boundary.dependency_violations(Path(workspace))

            self.assertEqual(len(violations), 1, violations)
            self.assertIn("core", violations[0])
            self.assertIn("ui", violations[0])

    def test_rejects_a_direct_internal_dependency_from_a_frontend(self):
        with tempfile.TemporaryDirectory() as workspace:
            crates = Path(workspace) / "crates"
            _write_manifest(crates, "ui", (
                '[package]\nname = "arclain_ui"\n\n'
                '[dependencies]\n'
                'arclain_core = { path = "../core" }\n'
            ))
            _write_manifest(crates, "core", '[package]\nname = "arclain_core"\n')

            violations = frontend_boundary.dependency_violations(Path(workspace))

            self.assertEqual(len(violations), 1, violations)
            self.assertIn("ui", violations[0])
            self.assertIn("core", violations[0])

    def test_accepts_the_sanctioned_frontend_dependency_on_app(self):
        # `app` (`arclain_app`) is the Stage 1 facade: a GUI crate
        # depending on it is the intended end state, not migration debt.
        with tempfile.TemporaryDirectory() as workspace:
            crates = Path(workspace) / "crates"
            _write_manifest(crates, "ui", (
                '[package]\nname = "arclain_ui"\n\n'
                '[dependencies]\n'
                'arclain_app = { path = "../app" }\n'
            ))
            _write_manifest(crates, "app", '[package]\nname = "arclain_app"\n')

            violations = frontend_boundary.dependency_violations(Path(workspace))

            self.assertEqual(violations, [])

    def test_still_rejects_other_headless_dependencies_alongside_the_sanctioned_one(self):
        # The `app` exemption is narrow: a frontend depending on `app`
        # *and* directly on `core` in the same manifest must still flag
        # the `core` edge.
        with tempfile.TemporaryDirectory() as workspace:
            crates = Path(workspace) / "crates"
            _write_manifest(crates, "ui", (
                '[package]\nname = "arclain_ui"\n\n'
                '[dependencies]\n'
                'arclain_app = { path = "../app" }\n'
                'arclain_core = { path = "../core" }\n'
            ))
            _write_manifest(crates, "app", '[package]\nname = "arclain_app"\n')
            _write_manifest(crates, "core", '[package]\nname = "arclain_core"\n')

            violations = frontend_boundary.dependency_violations(Path(workspace))

            self.assertEqual(len(violations), 1, violations)
            self.assertIn("ui", violations[0])
            self.assertIn("core", violations[0])

    def test_cli_depending_only_on_app_has_no_violations(self):
        # `cli` (arclain-cli) is a pure ArclainApp client: like a GUI
        # crate, it is only ever allowed to reach the headless world
        # through the sanctioned `app` facade dependency.
        with tempfile.TemporaryDirectory() as workspace:
            crates = Path(workspace) / "crates"
            _write_manifest(crates, "cli", (
                '[package]\nname = "arclain_cli"\n\n'
                '[dependencies]\n'
                'arclain_app = { path = "../app" }\n'
            ))
            _write_manifest(crates, "app", '[package]\nname = "arclain_app"\n')

            violations = frontend_boundary.dependency_violations(Path(workspace))

            self.assertEqual(violations, [])

    def test_cli_depending_directly_on_a_headless_crate_is_a_violation(self):
        with tempfile.TemporaryDirectory() as workspace:
            crates = Path(workspace) / "crates"
            _write_manifest(crates, "cli", (
                '[package]\nname = "arclain_cli"\n\n'
                '[dependencies]\n'
                'arclain_app = { path = "../app" }\n'
                'arclain_core = { path = "../core" }\n'
            ))
            _write_manifest(crates, "app", '[package]\nname = "arclain_app"\n')
            _write_manifest(crates, "core", '[package]\nname = "arclain_core"\n')

            violations = frontend_boundary.dependency_violations(Path(workspace))

            self.assertEqual(len(violations), 1, violations)
            self.assertIn("cli", violations[0])
            self.assertIn("core", violations[0])

    def test_cli_depending_directly_on_a_headless_crate_as_a_dev_dependency_is_a_violation(self):
        # The dev-dependencies table is checked too (see
        # _dependency_tables/_DEPENDENCY_TABLE_NAMES) -- a frontend crate
        # cannot reach a headless crate's internals from its own test
        # code either, which is exactly why arclain-cli's own read-surface
        # tests drive the compiled binary as a subprocess (or bootstrap
        # against `arclain_app` alone) instead of constructing e.g. an
        # `arclain_core::ArchiveBackend` fake directly.
        with tempfile.TemporaryDirectory() as workspace:
            crates = Path(workspace) / "crates"
            _write_manifest(crates, "cli", (
                '[package]\nname = "arclain_cli"\n\n'
                '[dependencies]\n'
                'arclain_app = { path = "../app" }\n\n'
                '[dev-dependencies]\n'
                'arclain_core = { path = "../core" }\n'
            ))
            _write_manifest(crates, "app", '[package]\nname = "arclain_app"\n')
            _write_manifest(crates, "core", '[package]\nname = "arclain_core"\n')

            violations = frontend_boundary.dependency_violations(Path(workspace))

            self.assertEqual(len(violations), 1, violations)
            self.assertIn("cli", violations[0])
            self.assertIn("dev-dependencies", violations[0])

    def test_headless_crate_depending_on_cli_is_a_violation(self):
        # The reverse edge must also be rejected: a headless crate has no
        # legitimate reason to depend on the CLI frontend.
        with tempfile.TemporaryDirectory() as workspace:
            crates = Path(workspace) / "crates"
            _write_manifest(crates, "core", (
                '[package]\nname = "arclain_core"\n\n'
                '[dependencies]\n'
                'arclain_cli = { path = "../cli" }\n'
            ))
            _write_manifest(crates, "cli", '[package]\nname = "arclain_cli"\n')

            violations = frontend_boundary.dependency_violations(Path(workspace))

            self.assertEqual(len(violations), 1, violations)
            self.assertIn("core", violations[0])
            self.assertIn("cli", violations[0])

    def test_accepts_the_declared_dependency_direction(self):
        with tempfile.TemporaryDirectory() as workspace:
            crates = Path(workspace) / "crates"
            # headless -> headless: core legitimately depends on data.
            _write_manifest(crates, "core", (
                '[package]\nname = "arclain_core"\n\n'
                '[dependencies]\n'
                'arclain_data = { path = "../data" }\n'
            ))
            _write_manifest(crates, "data", '[package]\nname = "arclain_data"\n')
            # GUI -> GUI: widgets legitimately depends on theme.
            _write_manifest(crates, "widgets", (
                '[package]\nname = "arclain_widgets"\n\n'
                '[dependencies]\n'
                'arclain_theme = { path = "../theme" }\n'
            ))
            _write_manifest(crates, "theme", '[package]\nname = "arclain_theme"\n')

            violations = frontend_boundary.dependency_violations(Path(workspace))

            self.assertEqual(violations, [])

    def test_inspects_normal_build_dev_and_target_specific_tables(self):
        with tempfile.TemporaryDirectory() as workspace:
            crates = Path(workspace) / "crates"
            _write_manifest(crates, "core", "\n".join((
                '[package]',
                'name = "arclain_core"',
                '',
                '[dependencies]',
                'arclain_ui_a = { path = "../ui" }',
                '',
                '[build-dependencies]',
                'arclain_ui_b = { path = "../ui" }',
                '',
                '[dev-dependencies]',
                'arclain_ui_c = { path = "../ui" }',
                '',
                "[target.'cfg(windows)'.dependencies]",
                'arclain_ui_d = { path = "../ui" }',
                '',
            )))
            _write_manifest(crates, "ui", '[package]\nname = "arclain_ui"\n')

            violations = frontend_boundary.dependency_violations(Path(workspace))

            self.assertEqual(len(violations), 4, violations)
            joined = "\n".join(violations)
            self.assertIn("dependencies", joined)
            self.assertIn("build-dependencies", joined)
            self.assertIn("dev-dependencies", joined)
            self.assertIn("target", joined)

    def test_non_path_dependencies_are_not_flagged(self):
        with tempfile.TemporaryDirectory() as workspace:
            crates = Path(workspace) / "crates"
            _write_manifest(crates, "core", (
                '[package]\nname = "arclain_core"\n\n'
                '[dependencies]\n'
                'egui = "0.33"\n'
                'serde = { version = "1", features = ["derive"] }\n'
            ))

            violations = frontend_boundary.dependency_violations(Path(workspace))

            self.assertEqual(violations, [])

    def test_missing_crates_directory_yields_no_violations(self):
        with tempfile.TemporaryDirectory() as workspace:
            violations = frontend_boundary.dependency_violations(Path(workspace))

        self.assertEqual(violations, [])


class TestSourceViolations(unittest.TestCase):
    def test_flags_fully_qualified_egui_reference_without_a_use_statement(self):
        with tempfile.TemporaryDirectory() as workspace:
            crate_dir = _write_manifest(
                Path(workspace) / "crates", "core", '[package]\nname = "arclain_core"\n',
            )
            _write_source_file(
                crate_dir, "lib.rs",
                "pub struct Wrapper {\n    ctx: egui::Context,\n}\n",
            )

            violations = frontend_boundary.source_violations(Path(workspace))

            self.assertEqual(len(violations), 1, violations)
            self.assertIn("egui", violations[0])
            self.assertIn("core", violations[0])

    def test_flags_eframe_reference(self):
        with tempfile.TemporaryDirectory() as workspace:
            crate_dir = _write_manifest(
                Path(workspace) / "crates", "core", '[package]\nname = "arclain_core"\n',
            )
            _write_source_file(
                crate_dir, "lib.rs",
                "fn update(frame: &mut eframe::Frame) {}\n",
            )

            violations = frontend_boundary.source_violations(Path(workspace))

            self.assertEqual(len(violations), 1, violations)
            self.assertIn("eframe", violations[0])

    def test_ignores_doc_comment_only_mention_of_eframe(self):
        # eframe appears only inside a `/// ```ignore` fenced example -- code
        # that documents a hypothetical caller and is never compiled. This
        # must not be flagged (regression: this exact shape previously
        # produced a fabricated violation from crates/signals/src/context.rs,
        # where the crate has no eframe dependency at all).
        with tempfile.TemporaryDirectory() as workspace:
            crate_dir = _write_manifest(
                Path(workspace) / "crates", "core", '[package]\nname = "arclain_core"\n',
            )
            _write_source_file(
                crate_dir, "lib.rs",
                "/// ```ignore\n"
                "/// fn update(frame: &mut eframe::Frame) {}\n"
                "/// ```\n"
                "pub fn noop() {}\n",
            )

            violations = frontend_boundary.source_violations(Path(workspace))

            self.assertEqual(violations, [])

    def test_doc_comment_egui_mention_does_not_shadow_the_real_code_reference(self):
        # A doc comment mentions egui in prose before the real code reference
        # appears. The reported line must be the real (non-comment) usage
        # site, not wherever the comment happened to be first.
        with tempfile.TemporaryDirectory() as workspace:
            crate_dir = _write_manifest(
                Path(workspace) / "crates", "core", '[package]\nname = "arclain_core"\n',
            )
            _write_source_file(
                crate_dir, "lib.rs",
                "//! Mentions egui in prose for docs only.\n"
                "\n"
                "pub struct Wrapper {\n"
                "    ctx: egui::Context,\n"
                "}\n",
            )

            violations = frontend_boundary.source_violations(Path(workspace))

            self.assertEqual(len(violations), 1, violations)
            self.assertIn("egui", violations[0])
            self.assertIn(":4:", violations[0])

    def test_flags_use_statement_referencing_a_gui_crate_name(self):
        with tempfile.TemporaryDirectory() as workspace:
            crate_dir = _write_manifest(
                Path(workspace) / "crates", "core", '[package]\nname = "arclain_core"\n',
            )
            _write_source_file(
                crate_dir, "lib.rs",
                "use arclain_ui::widget::Thing;\n\nfn noop() {}\n",
            )

            violations = frontend_boundary.source_violations(Path(workspace))

            self.assertEqual(len(violations), 1, violations)
            self.assertIn("arclain_ui", violations[0])

    def test_flags_use_statement_referencing_the_cli_frontend_crate_name(self):
        # arclain_cli is a FRONTEND_CRATES member, not a GUI_CRATES one,
        # but source_violations must still flag a headless crate
        # referencing it -- see source_violations's own FRONTEND_CRATES
        # loop.
        with tempfile.TemporaryDirectory() as workspace:
            crate_dir = _write_manifest(
                Path(workspace) / "crates", "core", '[package]\nname = "arclain_core"\n',
            )
            _write_source_file(
                crate_dir, "lib.rs",
                "use arclain_cli::commands::Cli;\n\nfn noop() {}\n",
            )

            violations = frontend_boundary.source_violations(Path(workspace))

            self.assertEqual(len(violations), 1, violations)
            self.assertIn("arclain_cli", violations[0])

    def test_ignores_doc_comment_mention_of_a_gui_crate_name(self):
        # arclain_ui shows up legitimately in headless doc comments describing
        # a *caller* in the GUI crate; that is not a real dependency and must
        # not be flagged (only an actual use/extern statement counts).
        with tempfile.TemporaryDirectory() as workspace:
            crate_dir = _write_manifest(
                Path(workspace) / "crates", "core", '[package]\nname = "arclain_core"\n',
            )
            _write_source_file(
                crate_dir, "lib.rs",
                "//! Called from arclain_ui::state::init at startup.\n\npub fn init() {}\n",
            )

            violations = frontend_boundary.source_violations(Path(workspace))

            self.assertEqual(violations, [])

    def test_flags_flutter_bridge_identifier_case_insensitively(self):
        with tempfile.TemporaryDirectory() as workspace:
            crate_dir = _write_manifest(
                Path(workspace) / "crates", "core", '[package]\nname = "arclain_core"\n',
            )
            _write_source_file(
                crate_dir, "lib.rs",
                "use Flutter_Rust_Bridge::bridge;\n",
            )

            violations = frontend_boundary.source_violations(Path(workspace))

            self.assertEqual(len(violations), 1, violations)

    def test_clean_headless_source_has_no_violations(self):
        with tempfile.TemporaryDirectory() as workspace:
            crate_dir = _write_manifest(
                Path(workspace) / "crates", "core", '[package]\nname = "arclain_core"\n',
            )
            _write_source_file(
                crate_dir, "lib.rs",
                "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
            )

            violations = frontend_boundary.source_violations(Path(workspace))

            self.assertEqual(violations, [])

    def test_gui_crates_are_not_scanned(self):
        with tempfile.TemporaryDirectory() as workspace:
            crate_dir = _write_manifest(
                Path(workspace) / "crates", "ui", '[package]\nname = "arclain_ui"\n',
            )
            _write_source_file(crate_dir, "lib.rs", "pub use egui::Context;\n")

            violations = frontend_boundary.source_violations(Path(workspace))

            self.assertEqual(violations, [])


class TestJustfileRecipes(unittest.TestCase):
    def test_justfile_has_frontend_boundary_recipes(self):
        text = (REPO_ROOT / "justfile").read_text(encoding="utf-8")

        self.assertIn("frontend-boundary:", text)
        self.assertIn("test-frontend-boundary:", text)
        self.assertIn("scripts/frontend_boundary.py", text)
        self.assertIn("scripts/test_frontend_boundary.py", text)


class TestCliEntryPoint(unittest.TestCase):
    def test_running_the_script_exits_zero_or_one_without_a_traceback(self):
        result = subprocess.run(
            [sys.executable, str(REPO_ROOT / "scripts" / "frontend_boundary.py")],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
        )

        self.assertIn(result.returncode, (0, 1))
        self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
