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
        return self.source_tree_violations({"src/lib.rs": source})

    def source_tree_violations(self, sources: dict[str, str]) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "wirt"
            crate.mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n',
                encoding="utf-8",
            )
            canonical = root / "wirt-sdk" / "wit"
            canonical.mkdir(parents=True)
            (canonical / "plugin.wit").write_text(
                "package wirt:plugin@0.1.0;\n", encoding="utf-8"
            )
            for relative, source in sources.items():
                path = crate / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(source, encoding="utf-8")
            return wirt_boundary.source_violations(root)

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
                wirt_boundary.dependency_violations(root),
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
                wirt_boundary.dependency_violations(root),
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
                wirt_boundary.dependency_violations(root),
                ["crates/wirt/Cargo.toml: forbidden dependency arclain_core"],
            )

    def test_renamed_wasmtime_dependency_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "wirt"
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n'
                '[dependencies]\nwasmtime = { package = "wasmtime-fork", version = "1" }\n',
                encoding="utf-8",
            )

            self.assertEqual(
                wirt_boundary.dependency_violations(root),
                [
                    "crates/wirt/Cargo.toml: wasmtime dependency must resolve "
                    "to package wasmtime"
                ],
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
                wirt_boundary.source_violations(root),
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
                wirt_boundary.source_violations(root),
                [
                    "crates/wirt/src/lib.rs:1: compiled source path escapes "
                    "crates/wirt/src: ../../../plugins/ui-demo/src/lib.rs"
                ],
            )

    def test_component_bindgen_path_must_use_the_canonical_wirt_sdk_wit(self):
        source = (
            "wasmtime::component::bindgen!({\n"
            '    path: "../../../plugins/ui-demo/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: component bindgen path must resolve to "
                "wirt-sdk/wit/plugin.wit: ../../../plugins/ui-demo/plugin.wit"
            ],
        )

    def test_second_wirt_plugin_wit_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical = root / "wirt-sdk" / "wit"
            legacy = root / "crates" / "wirt" / "wit"
            canonical.mkdir(parents=True)
            legacy.mkdir(parents=True)
            (root / "crates" / "wirt" / "Cargo.toml").write_text(
                '[package]\nname = "wirt"\nversion = "0.1.0"\n',
                encoding="utf-8",
            )
            (canonical / "plugin.wit").write_text(
                "package wirt:plugin@0.1.0;\n", encoding="utf-8"
            )
            (legacy / "plugin.wit").write_text(
                "package wirt:plugin@0.1.0;\n", encoding="utf-8"
            )

            self.assertEqual(
                wirt_boundary.wirt_wit_violations(root),
                [
                    "crates/wirt/wit/plugin.wit: unexpected plugin WIT; "
                    "only wirt-sdk/wit/plugin.wit is allowed"
                ],
            )

    def test_second_plugin_wit_is_rejected_even_for_another_package(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical = root / "wirt-sdk" / "wit"
            alternate = root / "other" / "plugin.wit"
            canonical.mkdir(parents=True)
            alternate.parent.mkdir()
            (canonical / "plugin.wit").write_text(
                "package wirt:plugin@0.1.0;\n", encoding="utf-8"
            )
            alternate.write_text("package example:plugin@1.0.0;\n", encoding="utf-8")

            self.assertEqual(
                wirt_boundary.wirt_wit_violations(root),
                [
                    "other/plugin.wit: unexpected plugin WIT; only "
                    "wirt-sdk/wit/plugin.wit is allowed"
                ],
            )

    def test_alternate_wirt_plugin_wit_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical = root / "wirt-sdk" / "wit"
            canonical.mkdir(parents=True)
            (canonical / "plugin.wit").write_text(
                "package wirt:plugin@0.1.0;\n", encoding="utf-8"
            )
            (root / "alternate.wit").write_text(
                "package wirt:plugin@0.1.0;\n", encoding="utf-8"
            )

            self.assertEqual(
                wirt_boundary.wirt_wit_violations(root),
                [
                    "alternate.wit: duplicate Wirt plugin WIT; only "
                    "wirt-sdk/wit/plugin.wit is allowed"
                ],
            )

    def test_alternate_wirt_plugin_namespace_at_another_version_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical = root / "wirt-sdk" / "wit"
            canonical.mkdir(parents=True)
            (canonical / "plugin.wit").write_text(
                "package wirt:plugin@0.1.0;\n", encoding="utf-8"
            )
            (root / "alternate.wit").write_text(
                "package wirt:plugin@0.2.0;\n", encoding="utf-8"
            )

            self.assertEqual(
                wirt_boundary.wirt_wit_violations(root),
                [
                    "alternate.wit: duplicate Wirt plugin WIT; only "
                    "wirt-sdk/wit/plugin.wit is allowed"
                ],
            )

    def test_whitespace_prefixed_wirt_plugin_package_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical = root / "wirt-sdk" / "wit"
            canonical.mkdir(parents=True)
            (canonical / "plugin.wit").write_text(
                "package wirt:plugin@0.1.0;\n", encoding="utf-8"
            )
            (root / "alternate.wit").write_text(
                "  package wirt:plugin@0.2.0;\n", encoding="utf-8"
            )

            self.assertEqual(
                wirt_boundary.wirt_wit_violations(root),
                [
                    "alternate.wit: duplicate Wirt plugin WIT; only "
                    "wirt-sdk/wit/plugin.wit is allowed"
                ],
            )

    def test_comment_separated_wirt_plugin_package_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical = root / "wirt-sdk" / "wit"
            canonical.mkdir(parents=True)
            (canonical / "plugin.wit").write_text(
                "package wirt:plugin@0.1.0;\n", encoding="utf-8"
            )
            (root / "alternate.wit").write_text(
                "// another WIT source\n"
                "package /* namespace */ wirt /* separator */ : plugin @ 0.2.0;\n",
                encoding="utf-8",
            )

            self.assertEqual(
                wirt_boundary.wirt_wit_violations(root),
                [
                    "alternate.wit: duplicate Wirt plugin WIT; only "
                    "wirt-sdk/wit/plugin.wit is allowed"
                ],
            )

    def test_malformed_wit_package_declaration_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical = root / "wirt-sdk" / "wit"
            canonical.mkdir(parents=True)
            (canonical / "plugin.wit").write_text(
                "package wirt:plugin@0.1.0;\n", encoding="utf-8"
            )
            (root / "alternate.wit").write_text(
                "package wirt:plugin@;\n", encoding="utf-8"
            )

            self.assertEqual(
                wirt_boundary.wirt_wit_violations(root),
                ["alternate.wit: malformed or ambiguous WIT package declaration"],
            )

    def test_ambiguous_wit_package_declarations_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical = root / "wirt-sdk" / "wit"
            canonical.mkdir(parents=True)
            (canonical / "plugin.wit").write_text(
                "package wirt:plugin@0.1.0;\n", encoding="utf-8"
            )
            (root / "alternate.wit").write_text(
                "package example:one@1.0.0;\n"
                "package example:two@1.0.0;\n",
                encoding="utf-8",
            )

            self.assertEqual(
                wirt_boundary.wirt_wit_violations(root),
                ["alternate.wit: malformed or ambiguous WIT package declaration"],
            )

    def test_missing_canonical_wirt_plugin_wit_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(
                wirt_boundary.wirt_wit_violations(Path(directory)),
                ["wirt-sdk/wit/plugin.wit: missing canonical Wirt plugin WIT"],
            )

    def test_imported_bindgen_alias_is_rejected(self):
        source = (
            "use wasmtime::component::bindgen as component_bindgen;\n"
            "component_bindgen!({\n"
            '    path: "../../../plugins/ui-demo/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            ["crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree"],
        )

    def test_imported_component_alias_is_rejected(self):
        source = (
            "use wasmtime::component as wasmtime_component;\n"
            "wasmtime_component::bindgen![{\n"
            '    path: "../../../plugins/ui-demo/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "}];\n"
        )

        self.assertEqual(
            self.source_violations(source),
            ["crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree"],
        )

    def test_grouped_component_alias_is_rejected(self):
        source = (
            "use wasmtime::{component as component_alias};\n"
            "component_alias::bindgen! {\n"
            '    path: "../../wirt-sdk/wit/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "}\n"
        )

        self.assertEqual(
            self.source_violations(source),
            ["crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree"],
        )

    def test_grouped_component_import_path_is_validated(self):
        source = (
            "use wasmtime::{component};\n"
            "component::bindgen!({\n"
            '    path: "../../../plugins/ui-demo/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            ["crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree"],
        )

    def test_grouped_bindgen_import_path_is_validated(self):
        source = (
            "use wasmtime::component::{bindgen};\n"
            "bindgen![{\n"
            '    path: "../../../plugins/ui-demo/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "}];\n"
        )

        self.assertEqual(
            self.source_violations(source),
            ["crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree"],
        )

    def test_nested_grouped_bindgen_alias_path_is_validated(self):
        source = (
            "use wasmtime::{component::{bindgen as component_bindgen}};\n"
            "component_bindgen!({\n"
            '    path: "../../../plugins/ui-demo/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            ["crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree"],
        )

    def test_wasmtime_root_alias_path_is_validated(self):
        source = (
            "use wasmtime as wt;\n"
            "wt::component::bindgen!({\n"
            '    path: "../../../plugins/ui-demo/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            ["crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree"],
        )

    def test_absolute_wasmtime_root_alias_path_is_validated(self):
        source = (
            "use ::wasmtime as wt;\n"
            "wt::component::bindgen!({\n"
            '    path: "../../../plugins/ui-demo/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            ["crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree"],
        )

    def test_wasmtime_glob_import_fails_closed(self):
        source = (
            "use wasmtime::*;\n"
            "bindgen!({\n"
            '    path: "../../wirt-sdk/wit/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime "
                "component use tree",
            ],
        )

    def test_component_glob_import_fails_closed(self):
        source = (
            "use wasmtime::component::*;\n"
            "bindgen!({\n"
            '    path: "../../wirt-sdk/wit/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime "
                "component use tree",
            ],
        )

    def test_nested_component_glob_import_fails_closed(self):
        source = (
            "use wasmtime::{component::*};\n"
            "bindgen!({\n"
            '    path: "../../wirt-sdk/wit/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime "
                "component use tree",
            ],
        )

    def test_extern_wasmtime_alias_path_is_validated(self):
        source = (
            "extern crate wasmtime as wt;\n"
            "wt::component::bindgen!({\n"
            '    path: "../../../plugins/ui-demo/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            ["crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree"],
        )

    def test_ordinary_wasmtime_type_imports_and_plain_extern_are_allowed(self):
        source = (
            "use wasmtime::component::Component as WasmComponent;\n"
            "use wasmtime::Engine as WasmEngine;\n"
            "extern crate wasmtime;\n"
        )

        self.assertEqual(self.source_violations(source), [])

    def test_local_wasmtime_module_shadow_is_rejected(self):
        source = (
            "mod wasmtime {}\n"
            "wasmtime::component::bindgen!({\n"
            '    path: "../../wirt-sdk/wit/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            ["crates/wirt/src/lib.rs:1: local wasmtime module declaration is not allowed"],
        )

    def test_unresolved_component_bindgen_is_rejected(self):
        source = (
            "component::bindgen!({\n"
            '    path: "../../wirt-sdk/wit/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: unsupported or ambiguous component "
                "bindgen macro path: component::bindgen"
            ],
        )

    def test_unresolved_bindgen_is_rejected(self):
        source = (
            "bindgen!({\n"
            '    path: "../../wirt-sdk/wit/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: unsupported or ambiguous component "
                "bindgen macro path: bindgen"
            ],
        )

    def test_ambiguous_component_reexports_fail_closed(self):
        source = (
            "pub use wasmtime::component;\n"
            "pub use crate::other::component;\n"
            "component::bindgen!({\n"
            '    path: "../../wirt-sdk/wit/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime "
                "component use tree",
                "crates/wirt/src/lib.rs:2: unsupported or ambiguous Wasmtime "
                "component use tree",
            ],
        )

    def test_second_bindgen_path_is_rejected(self):
        source = (
            "wasmtime::component::bindgen!({\n"
            '    path: "../../wirt-sdk/wit/plugin.wit",\n'
            '    path: "../../../plugins/ui-demo/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: component bindgen has duplicate input "
                "field: path"
            ],
        )

    def test_duplicate_bindgen_field_is_rejected(self):
        source = (
            "wasmtime::component::bindgen!({\n"
            '    path: "../../wirt-sdk/wit/plugin.wit",\n'
            '    world: "plugin-world",\n'
            '    world: "another-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: component bindgen has duplicate input "
                "field: world"
            ],
        )

    def test_inline_bindgen_source_is_rejected_alongside_canonical_path(self):
        source = (
            "wasmtime::component::bindgen!({\n"
            '    path: "../../wirt-sdk/wit/plugin.wit",\n'
            '    inline: "package evil:plugin;",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: component bindgen must use exactly one "
                "literal path input"
            ],
        )

    def test_bindgen_path_list_is_rejected(self):
        source = (
            "wasmtime::component::bindgen!({\n"
            '    path: ["../../wirt-sdk/wit/plugin.wit"],\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: component bindgen must use exactly one "
                "literal path input"
            ],
        )

    def test_interfaces_bindgen_source_is_rejected(self):
        source = (
            "wasmtime::component::bindgen!({\n"
            '    interfaces: "interface evil {}",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: component bindgen must use exactly one "
                "literal path input"
            ],
        )

    def test_bindgen_source_shorthand_is_rejected(self):
        source = (
            "wasmtime::component::bindgen!(\n"
            '    "plugin-world" in "../../../plugins/ui-demo/plugin.wit"\n'
            ");\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: component bindgen must use an inner braced "
                "argument map"
            ],
        )

    def test_dynamic_bindgen_path_is_rejected(self):
        source = (
            "wasmtime::component::bindgen!({\n"
            "    path: include_str!(\"../../../plugins/ui-demo/plugin.wit\"),\n"
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: component bindgen must use exactly one "
                "literal path input"
            ],
        )

    def test_nested_wirt_source_resolves_bindgen_path_from_crate_root(self):
        source = (
            "wasmtime::component::bindgen!({\n"
            '    path: "../../wirt-sdk/wit/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_tree_violations({"src/nested/lib.rs": source}),
            [],
        )

    def test_public_bindgen_reexport_is_rejected_at_its_definition(self):
        source = (
            "pub use wasmtime::component::bindgen as wb;\n"
            "wb!({\n"
            '    path: "../../../plugins/ui-demo/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_tree_violations({"src/shim.rs": source}),
            [
                "crates/wirt/src/shim.rs:1: unsupported or ambiguous Wasmtime component use tree"
            ],
        )

    def test_public_component_reexport_is_rejected_at_its_definition(self):
        source = "pub use wasmtime::component;\n"

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree"
            ],
        )

    def test_public_wasmtime_root_reexport_is_rejected_at_its_definition(self):
        source = "pub use wasmtime as wt;\n"

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree"
            ],
        )

    def test_public_wasmtime_component_glob_reexport_is_rejected(self):
        source = "pub use wasmtime::component::*;\n"

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree",
            ],
        )

    def test_cross_file_public_bindgen_alias_is_rejected(self):
        sources = {
            "src/lib.rs": (
                "use crate::shim::wb;\n"
                "wb!({\n"
                '    path: "../../../plugins/ui-demo/plugin.wit",\n'
                '    world: "plugin-world",\n'
                "});\n"
            ),
            "src/shim.rs": "pub use wasmtime::component::bindgen as wb;\n",
        }

        self.assertEqual(
            self.source_tree_violations(sources),
            [
                "crates/wirt/src/shim.rs:1: unsupported or ambiguous Wasmtime component use tree"
            ],
        )

    def test_non_wasmtime_reserved_alias_is_rejected(self):
        source = (
            "use crate::shim as wasmtime;\n"
            "wasmtime::component::bindgen!({\n"
            '    path: "../../../plugins/ui-demo/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree",
            ],
        )

    def test_non_wasmtime_extern_reserved_alias_is_rejected(self):
        source = (
            "extern crate shim as wasmtime;\n"
            "wasmtime::component::bindgen!({\n"
            '    path: "../../../plugins/ui-demo/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime component use tree",
            ],
        )

    def test_malformed_bindgen_argument_map_is_rejected(self):
        source = (
            "wasmtime::component::bindgen!({\n"
            '    path: "../../wirt-sdk/wit/plugin.wit"\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: component bindgen must use exactly one "
                "literal path input"
            ],
        )

    def test_unterminated_wasmtime_import_fails_closed(self):
        source = (
            "use wasmtime as wt\n"
            "wt::component::bindgen!({\n"
            '    path: "../../wirt-sdk/wit/plugin.wit",\n'
            '    world: "plugin-world",\n'
            "});\n"
        )

        self.assertEqual(
            self.source_violations(source),
            [
                "crates/wirt/src/lib.rs:1: unsupported or ambiguous Wasmtime "
                "component use tree",
            ],
        )

    def test_all_outer_delimiters_accept_an_inner_bindgen_map(self):
        sources = {
            "src/lib.rs": (
                "wasmtime::component::bindgen!({ path: \"../../wirt-sdk/wit/plugin.wit\" });\n"
            ),
            "src/one.rs": (
                "wasmtime::component::bindgen![{ path: \"../../wirt-sdk/wit/plugin.wit\" }];\n"
            ),
            "src/two.rs": (
                "wasmtime::component::bindgen!{{ path: \"../../wirt-sdk/wit/plugin.wit\" }}\n"
            ),
        }

        self.assertEqual(self.source_tree_violations(sources), [])

    def test_outer_brace_is_not_a_bindgen_argument_map(self):
        source = 'wasmtime::component::bindgen!{ path: "../../wirt-sdk/wit/plugin.wit" }\n'

        self.assertEqual(
            self.source_violations(source),
            ["crates/wirt/src/lib.rs:1: component bindgen must use an inner braced argument map"],
        )

    def test_all_regular_source_root_files_are_scanned_for_bindgen(self):
        sources = {
            "src/lib.rs": "pub struct Neutral;\n",
            "src/dormant.inc": (
                "wasmtime::component::bindgen!({\n"
                '  path: "../../../plugins/ui-demo/plugin.wit",\n'
                "});\n"
            ),
        }

        self.assertEqual(
            self.source_tree_violations(sources),
            [
                "crates/wirt/src/dormant.inc:1: component bindgen path must resolve to "
                "wirt-sdk/wit/plugin.wit: ../../../plugins/ui-demo/plugin.wit"
            ],
        )

    def test_bracket_and_brace_includes_do_not_hide_non_rs_bindgen(self):
        sources = {
            "src/lib.rs": (
                'include!["bracket.inc"];\n'
                'include!{ "brace.inc" };\n'
            ),
            "src/bracket.inc": (
                "wasmtime::component::bindgen!({\n"
                '  path: "../../../plugins/ui-demo/plugin.wit",\n'
                "});\n"
            ),
            "src/brace.inc": (
                "wasmtime::component::bindgen!({\n"
                '  path: "../../../plugins/ui-demo/plugin.wit",\n'
                "});\n"
            ),
        }

        self.assertEqual(
            self.source_tree_violations(sources),
            [
                "crates/wirt/src/brace.inc:1: component bindgen path must resolve to "
                "wirt-sdk/wit/plugin.wit: ../../../plugins/ui-demo/plugin.wit",
                "crates/wirt/src/bracket.inc:1: component bindgen path must resolve to "
                "wirt-sdk/wit/plugin.wit: ../../../plugins/ui-demo/plugin.wit",
            ],
        )

    def test_inline_module_decoy_does_not_hide_actual_source_root_target(self):
        sources = {
            "src/lib.rs": 'mod generated { include!["bindings.inc"]; }\n',
            "src/generated/bindings.inc": "const DECOY: u8 = 1;\n",
            "src/bindings.inc": (
                "wasmtime::component::bindgen!({\n"
                '  path: "../../../plugins/ui-demo/plugin.wit",\n'
                "});\n"
            ),
        }

        self.assertEqual(
            self.source_tree_violations(sources),
            [
                "crates/wirt/src/bindings.inc:1: component bindgen path must resolve to "
                "wirt-sdk/wit/plugin.wit: ../../../plugins/ui-demo/plugin.wit"
            ],
        )

    def test_included_non_rs_source_is_scanned_for_bindgen(self):
        sources = {
            "src/lib.rs": 'include!("bindings.inc");\n',
            "src/bindings.inc": (
                "wasmtime::component::bindgen!({\n"
                '  path: "../../../plugins/ui-demo/plugin.wit",\n'
                "});\n"
            ),
        }

        self.assertEqual(
            self.source_tree_violations(sources),
            [
                "crates/wirt/src/bindings.inc:1: component bindgen path must resolve to "
                "wirt-sdk/wit/plugin.wit: ../../../plugins/ui-demo/plugin.wit"
            ],
        )

    def test_each_bindgen_in_included_non_rs_source_is_checked(self):
        sources = {
            "src/lib.rs": 'include!("bindings.inc");\n',
            "src/bindings.inc": (
                "wasmtime::component::bindgen!({\n"
                '    path: "../../../plugins/ui-demo/plugin.wit",\n'
                "});\n\n"
                "wasmtime::component::bindgen!({\n"
                '    path: "../../../plugins/ui-demo/plugin.wit",\n'
                "});\n"
            ),
        }

        self.assertEqual(
            self.source_tree_violations(sources),
            [
                "crates/wirt/src/bindings.inc:1: component bindgen path must resolve to "
                "wirt-sdk/wit/plugin.wit: ../../../plugins/ui-demo/plugin.wit",
                "crates/wirt/src/bindings.inc:5: component bindgen path must resolve to "
                "wirt-sdk/wit/plugin.wit: ../../../plugins/ui-demo/plugin.wit",
            ],
        )

    def test_included_non_rs_source_without_bindgen_is_allowed(self):
        sources = {
            "src/lib.rs": 'include!("bindings.inc");\n',
            "src/bindings.inc": "const INCLUDED: u8 = 1;\n",
        }

        self.assertEqual(self.source_tree_violations(sources), [])

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
                wirt_boundary.source_violations(root),
                [
                    "crates/wirt/src/lib.rs:1: compiled source path escapes "
                    "crates/wirt/src: ../../../plugins/ui-demo/src/lib.rs"
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
                wirt_boundary.source_violations(root),
                [
                    "crates/wirt/src/lib.rs:1: compiled source path escapes "
                    "crates/wirt/src: ../../../plugins/ui-demo/src/lib.rs"
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
                wirt_boundary.source_violations(root),
                ["crates/wirt/src/lib.rs:1: include! path is not a string literal"],
            )

    def test_path_attribute_escaping_wirt_source_root_is_rejected(self):
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

            self.assertEqual(
                wirt_boundary.source_violations(root),
                [
                    "crates/wirt/src/runtime/tests.rs:1: compiled source path escapes "
                    "crates/wirt/src: ../../tests/support/stub_host.rs"
                ],
            )

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

            self.assertEqual(wirt_boundary.source_violations(root), [])

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
            "crates/wirt/src: ../../../plugins/ui-demo/src/lib.rs"
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
            ("upper byte character escape", r"const VALUE: u8 = b'\xFF';"),
            (
                "byte string",
                r'''const VALUE: &[u8] = b"include!(\"../../../plugins/ui-demo/src/lib.rs\")";''',
            ),
            (
                "upper byte string escapes",
                r'''const VALUE: &[u8] = b"\x80\xFF";''',
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
                "crates/wirt/src: ../../../plugins/ui-demo/src/lib.rs"
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
            (
                "Unicode escape in byte character",
                r"const VALUE: u8 = b'\u{41}';",
                "malformed byte character literal",
            ),
            (
                "Unicode escape in byte string",
                r'''const VALUE: &[u8] = b"\u{41}";''',
                "malformed byte string literal",
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
            r"const UPPER_BYTE_CHARACTER: u8 = b'\xFF';"
            "\n"
            r'''const BYTE_STRING: &[u8] = b"harmless \" quote \\ slash";'''
            "\n"
            r'''const UPPER_BYTE_STRING: &[u8] = b"\x80\xFF";'''
            "\n"
            r'''const RAW_BYTE_STRING: &[u8] = br#"harmless " quote"#;'''
            "\n"
            r'''const C_STRING: &core::ffi::CStr = c"harmless \" quote";'''
            "\n"
            r'''const RAW_C_STRING: &core::ffi::CStr = cr#"harmless " quote"#;'''
            "\n"
            "fn identity<'a>(value: &'a str) -> &'a str { "
            "'label: { break 'label value } }\n"
            r"const UNICODE_CHARACTER: char = '\u{1F980}';"
            "\n"
            r'''const UNICODE_STRING: &str = "\u{1F980}";'''
            "\n"
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
