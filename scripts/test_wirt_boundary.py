#!/usr/bin/env python3
"""Contract tests for Wirt's product-neutral dependency boundary."""
from __future__ import annotations

import subprocess
import shutil
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import wirt_boundary

REPO_ROOT = Path(__file__).resolve().parents[1]


class TestWirtBoundary(unittest.TestCase):
    def source_violations(self, source: str) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "wirt"
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n',
                encoding="utf-8",
            )
            (crate / "src" / "lib.rs").write_text(source, encoding="utf-8")
            return wirt_boundary.violations(root)

    def test_app_manifest_declares_neutral_model_and_product_adapter_edges(self):
        with (REPO_ROOT / "crates" / "app" / "Cargo.toml").open("rb") as handle:
            dependencies = tomllib.load(handle)["dependencies"]

        self.assertEqual(dependencies["wirt"]["path"], "../wirt")
        self.assertEqual(dependencies["arclain_plugins"]["path"], "../plugins")

    def test_plugins_manifest_has_no_native_dialog_dependency(self):
        with (REPO_ROOT / "crates" / "plugins" / "Cargo.toml").open(
            "rb"
        ) as handle:
            dependencies = tomllib.load(handle)["dependencies"]

        self.assertNotIn("rfd", dependencies)

    def test_secure_loader_ownership_is_structural(self):
        neutral_loader = REPO_ROOT / "crates" / "wirt" / "src" / "loader" / "mod.rs"
        neutral_tests = REPO_ROOT / "crates" / "wirt" / "src" / "loader" / "tests.rs"
        product_tests = REPO_ROOT / "crates" / "plugins" / "src" / "loader" / "tests.rs"

        self.assertTrue(neutral_loader.is_file())
        self.assertTrue(neutral_tests.is_file())
        self.assertFalse(product_tests.exists())

        with (REPO_ROOT / "crates" / "wirt" / "Cargo.toml").open("rb") as handle:
            dependencies = tomllib.load(handle)["dependencies"]
        self.assertEqual(dependencies["cap-std"], "=4.0.2")
        self.assertEqual(dependencies["cap-fs-ext"], "=4.0.2")

    def test_product_manager_stays_out_of_wirt(self):
        self.assertTrue(
            (REPO_ROOT / "crates" / "plugins" / "src" / "manager" / "mod.rs").is_file()
        )
        self.assertFalse((REPO_ROOT / "crates" / "wirt" / "src" / "manager.rs").exists())
        self.assertFalse((REPO_ROOT / "crates" / "wirt" / "src" / "manager").exists())

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

    def test_workspace_inherited_product_alias_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "wirt"
            (crate / "src").mkdir(parents=True)
            (root / "Cargo.toml").write_text(
                '[workspace]\nmembers = ["crates/wirt"]\n'
                '[workspace.dependencies]\n'
                'neutral = { package = "arclain_core", path = "crates/core" }\n',
                encoding="utf-8",
            )
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n'
                '[dependencies]\nneutral.workspace = true\n',
                encoding="utf-8",
            )
            (crate / "src" / "lib.rs").write_text(
                "pub struct Neutral;\n", encoding="utf-8"
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

    def test_path_attribute_escaping_wirt_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "wirt"
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n',
                encoding="utf-8",
            )
            (crate / "src" / "lib.rs").write_text(
                '#[path = "../../../plugins/ui-demo/src/lib.rs"]\nmod product;\n',
                encoding="utf-8",
            )

            self.assertEqual(
                wirt_boundary.violations(root),
                [
                    "crates/wirt/src/lib.rs:1: compiled source path escapes "
                    "crates/wirt: ../../../plugins/ui-demo/src/lib.rs"
                ],
            )

    def test_literal_include_escaping_wirt_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "wirt"
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n',
                encoding="utf-8",
            )
            (crate / "src" / "lib.rs").write_text(
                'include!(r"../../../plugins/ui-demo/src/lib.rs");\n',
                encoding="utf-8",
            )

            self.assertEqual(
                wirt_boundary.violations(root),
                [
                    "crates/wirt/src/lib.rs:1: compiled source path escapes "
                    "crates/wirt: ../../../plugins/ui-demo/src/lib.rs"
                ],
            )

    def test_cfg_attr_path_escaping_wirt_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "wirt"
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n',
                encoding="utf-8",
            )
            (crate / "src" / "lib.rs").write_text(
                '#[cfg_attr(test, path = "../../../plugins/ui-demo/src/lib.rs")]\n'
                "mod product;\n",
                encoding="utf-8",
            )

            self.assertEqual(
                wirt_boundary.violations(root),
                [
                    "crates/wirt/src/lib.rs:1: compiled source path escapes "
                    "crates/wirt: ../../../plugins/ui-demo/src/lib.rs"
                ],
            )

    def test_dynamic_include_is_rejected_when_confinement_cannot_be_proven(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "wirt"
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n',
                encoding="utf-8",
            )
            (crate / "src" / "lib.rs").write_text(
                'include!(concat!("../", "generated.rs"));\n',
                encoding="utf-8",
            )

            self.assertEqual(
                wirt_boundary.violations(root),
                ["crates/wirt/src/lib.rs:1: include! path is not a string literal"],
            )

    def test_path_attribute_resolving_inside_wirt_is_allowed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "wirt"
            (crate / "src" / "runtime").mkdir(parents=True)
            (crate / "tests" / "support").mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n',
                encoding="utf-8",
            )
            (crate / "src" / "runtime" / "tests.rs").write_text(
                '#[path = "../../tests/support/stub_host.rs"]\nmod stub_host;\n',
                encoding="utf-8",
            )
            (crate / "tests" / "support" / "stub_host.rs").write_text(
                "pub struct StubHost;\n", encoding="utf-8"
            )

            self.assertEqual(wirt_boundary.violations(root), [])

    def test_comments_and_string_literals_with_code_spellings_are_ignored(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "wirt"
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n',
                encoding="utf-8",
            )
            (crate / "src" / "lib.rs").write_text(
                "/*\n"
                "use arclain_core::Service;\n"
                '#[path = "../../../plugins/ui-demo/src/lib.rs"]\n'
                'include!("../../../plugins/ui-demo/src/lib.rs");\n'
                "*/\n"
                'const NORMAL: &str = "include!(\\\"../../../plugins/ui-demo/src/lib.rs\\\")";\n'
                'const RAW: &str = r###"#[path = "../../../plugins/ui-demo/src/lib.rs"]"###;\n',
                encoding="utf-8",
            )

            self.assertEqual(wirt_boundary.violations(root), [])

    def test_opaque_literal_matrix_does_not_hide_a_following_include(self):
        literals = (
            ("character", "const VALUE: char = '\"';"),
            ("byte character", "const VALUE: u8 = b'\"';"),
            ("byte string", r'''const VALUE: &[u8] = b"harmless \" quote \\ slash";'''),
            ("raw byte string", r'''const VALUE: &[u8] = br#"harmless " quote"#;'''),
            (
                "C string",
                r'''const VALUE: &core::ffi::CStr = c"harmless \" quote";''',
            ),
            (
                "raw C string",
                r'''const VALUE: &core::ffi::CStr = cr#"harmless " quote"#;''',
            ),
        )
        expected = [
            "crates/wirt/src/lib.rs:2: compiled source path escapes "
            "crates/wirt: ../../../plugins/ui-demo/src/lib.rs"
        ]

        for name, literal in literals:
            with self.subTest(literal=name):
                source = (
                    f"{literal}\n"
                    'include!("../../../plugins/ui-demo/src/lib.rs");\n'
                )
                self.assertEqual(self.source_violations(source), expected)

    def test_opaque_literal_contents_alone_have_no_boundary_meaning(self):
        literals = (
            ("character", "const VALUE: char = '\"';"),
            ("byte character", "const VALUE: u8 = b'\"';"),
            (
                "byte string",
                r'''const VALUE: &[u8] = b"include!(\"../../../plugins/ui-demo/src/lib.rs\")";''',
            ),
            (
                "raw byte string",
                r'''const VALUE: &[u8] = br#"include!("../../../plugins/ui-demo/src/lib.rs")"#;''',
            ),
            (
                "C string",
                r'''const VALUE: &core::ffi::CStr = c"include!(\"../../../plugins/ui-demo/src/lib.rs\")";''',
            ),
            (
                "raw C string",
                r'''const VALUE: &core::ffi::CStr = cr#"include!("../../../plugins/ui-demo/src/lib.rs")"#;''',
            ),
        )

        for name, literal in literals:
            with self.subTest(literal=name):
                self.assertEqual(self.source_violations(f"{literal}\n"), [])

    def test_lifetimes_and_labels_do_not_desynchronize_following_code(self):
        source = (
            "fn identity<'a>(value: &'a str) -> &'a str { "
            "'label: { break 'label value } }\n"
            'include!("../../../plugins/ui-demo/src/lib.rs");\n'
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:2: compiled source path escapes "
                "crates/wirt: ../../../plugins/ui-demo/src/lib.rs"
            ],
        )

    def test_unterminated_or_malformed_lexical_forms_fail_closed(self):
        cases = (
            ("block comment", "/* never closes", "unterminated block comment"),
            (
                "normal string",
                'const VALUE: &str = "never closes',
                "unterminated string literal",
            ),
            (
                "raw string",
                'const VALUE: &str = r#"never closes',
                "unterminated raw string literal",
            ),
            (
                "character",
                "const VALUE: char = '\"",
                "unterminated character literal",
            ),
            (
                "byte character",
                "const VALUE: u8 = b'x",
                "unterminated byte character literal",
            ),
            (
                "byte string",
                'const VALUE: &[u8] = b"never closes',
                "unterminated byte string literal",
            ),
            (
                "raw byte string",
                'const VALUE: &[u8] = br#"never closes',
                "unterminated raw byte string literal",
            ),
            (
                "C string",
                'const VALUE: &core::ffi::CStr = c"never closes',
                "unterminated C string literal",
            ),
            (
                "raw C string",
                'const VALUE: &core::ffi::CStr = cr#"never closes',
                "unterminated raw C string literal",
            ),
            (
                "invalid escape",
                'const VALUE: &str = "bad \\q escape";',
                "malformed string literal",
            ),
            (
                "oversized character",
                "const VALUE: char = 'ab';",
                "malformed character literal",
            ),
        )

        for name, source, message in cases:
            with self.subTest(lexical_form=name):
                self.assertEqual(
                    self.source_violations(source),
                    [f"crates/wirt/src/lib.rs:1: lexical error: {message}"],
                )

    def test_literal_matrix_is_valid_rust(self):
        rustc = shutil.which("rustc")
        if rustc is None:
            self.skipTest("rustc is required to validate the Rust literal fixture")
        source = (
            "const CHARACTER: char = '\"';\n"
            "const BYTE_CHARACTER: u8 = b'\"';\n"
            r'''const BYTE_STRING: &[u8] = b"harmless \" quote \\ slash";'''
            "\n"
            r'''const RAW_BYTE_STRING: &[u8] = br#"harmless " quote"#;'''
            "\n"
            r'''const C_STRING: &core::ffi::CStr = c"harmless \" quote";'''
            "\n"
            r'''const RAW_C_STRING: &core::ffi::CStr = cr#"harmless " quote"#;'''
            "\n"
            "fn identity<'a>(value: &'a str) -> &'a str { "
            "'label: { break 'label value } }\n"
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / "literal_fixture.rs"
            fixture.write_text(source, encoding="utf-8")
            result = subprocess.run(
                [
                    rustc,
                    "--crate-type",
                    "lib",
                    "--edition",
                    "2021",
                    "--emit",
                    "metadata",
                    "-o",
                    str(root / "literal_fixture.rmeta"),
                    str(fixture),
                ],
                capture_output=True,
                text=True,
            )

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

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
