from __future__ import annotations

import unittest

from scripts._check_plugin_host import (
    plugin_facade_gate_errors,
    plugin_host_rustdoc_errors,
    tree_contract_errors,
)


class TestPluginHostTreeContract(unittest.TestCase):
    def test_accepts_default_positive_controls_and_a_clean_archive_tree(self):
        default_tree = "\n".join(
            (
                "arclain_app v0.1.0",
                "├── arclain_plugins v0.1.0",
                "│   └── wirt v0.3.0",
                "│       └── wasmtime v35.0.0",
            )
        )

        self.assertEqual(tree_contract_errors(default_tree, "arclain_app v0.1.0"), [])

    def test_rejects_every_plugin_host_package_in_the_archive_tree(self):
        default_tree = "\n".join(
            (
                "arclain_app v0.1.0",
                "├── arclain_plugins v0.1.0",
                "│   └── wirt v0.3.0",
                "│       └── wasmtime v35.0.0",
            )
        )
        archive_tree = "\n".join(
            (
                "arclain_app v0.1.0",
                "├── arclain_plugins v0.1.0",
                "├── wirt v0.3.0",
                "└── wasmtime v35.0.0",
            )
        )

        self.assertEqual(
            tree_contract_errors(default_tree, archive_tree),
            [
                "archive-only tree contains arclain_plugins",
                "archive-only tree contains wirt",
                "archive-only tree contains wasmtime",
            ],
        )

    def test_rejects_a_missing_default_positive_control(self):
        default_tree = "\n".join(
            (
                "arclain_app v0.1.0",
                "├── arclain_plugins v0.1.0",
                "└── wirt v0.3.0",
            )
        )

        self.assertEqual(
            tree_contract_errors(default_tree, "arclain_app v0.1.0"),
            ["default tree is missing wasmtime"],
        )

    def test_checkout_path_names_are_not_treated_as_packages(self):
        path_only_tree = (
            "arclain_app v0.1.0 "
            "(C:/work/arclain_plugins checkout/wirt checkout/wasmtime checkout)"
        )

        self.assertEqual(
            tree_contract_errors(path_only_tree, "arclain_app v0.1.0"),
            [
                "default tree is missing arclain_plugins",
                "default tree is missing wirt",
                "default tree is missing wasmtime",
            ],
        )


class TestPluginFacadeSourceContract(unittest.TestCase):
    def test_accepts_a_compile_time_gated_plugin_method(self):
        source = '''
impl ArclainApp {
    #[cfg(feature = "plugin-host")]
    pub async fn plugins(&self) {}
}
'''

        self.assertEqual(plugin_facade_gate_errors(source, ("plugins",)), [])

    def test_rejects_an_ungated_plugin_method(self):
        source = '''
impl ArclainApp {
    pub async fn plugins(&self) {}
}
'''

        self.assertEqual(
            plugin_facade_gate_errors(source, ("plugins",)),
            ["plugin facade method `plugins` is not feature-gated"],
        )


class TestPluginHostRustdocContract(unittest.TestCase):
    def test_reports_only_broken_links_to_feature_gated_plugin_items(self):
        diagnostics = "\n".join(
            (
                "error: unresolved link to `detect`",
                "error: unresolved link to `crate::analyze_url`",
                "error: unresolved link to `crate::plugins::ArchiveContextBridge`",
                "error: public documentation links to private item `run_extract`",
            )
        )

        self.assertEqual(
            plugin_host_rustdoc_errors(diagnostics),
            [
                "error: unresolved link to `crate::analyze_url`",
                "error: unresolved link to `crate::plugins::ArchiveContextBridge`",
            ],
        )


if __name__ == "__main__":
    unittest.main()
