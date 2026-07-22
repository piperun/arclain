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
import inspect
import json
import os
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import _package
import _ui
from _package import get_platform, workspace_version
from _ui import load_rust_log

REPO_ROOT = Path(__file__).resolve().parents[1]


class TestPluginVersions(unittest.TestCase):
    def test_plugin_manifest_versions_match_cargo(self):
        for plugin in ("dlsite-metadata", "gstreamer-preview", "ui-demo"):
            with self.subTest(plugin=plugin):
                root = REPO_ROOT / "plugins" / plugin
                with (root / "Cargo.toml").open("rb") as handle:
                    cargo_version = tomllib.load(handle)["package"]["version"]
                with (root / f"{plugin}.toml").open("rb") as handle:
                    manifest_version = tomllib.load(handle)["plugin"]["version"]
                self.assertEqual(manifest_version, cargo_version, plugin)


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


class TestPackageTargetDir(unittest.TestCase):
    def test_cargo_target_dir_uses_env_override(self):
        with tempfile.TemporaryDirectory() as d:
            target_dir = Path(d) / "custom-target"
            with mock.patch.dict(
                os.environ,
                {"CARGO_TARGET_DIR": str(target_dir)},
                clear=False,
            ):
                self.assertEqual(_package.cargo_target_dir(Path(d)), target_dir)

    def test_cargo_target_dir_reads_cargo_metadata(self):
        with tempfile.TemporaryDirectory() as d:
            repo_root = Path(d)
            target_dir = repo_root / "metadata-target"
            result = subprocess.CompletedProcess(
                ["cargo", "metadata"],
                0,
                stdout=json.dumps({"target_directory": str(target_dir)}),
                stderr="",
            )
            with mock.patch.dict(os.environ, {}, clear=True), \
                 mock.patch.object(
                     _package.subprocess,
                     "run",
                     return_value=result,
                 ) as run:
                self.assertEqual(_package.cargo_target_dir(repo_root), target_dir)

            run.assert_called_once_with(
                ["cargo", "metadata", "--no-deps", "--format-version", "1"],
                cwd=repo_root,
                capture_output=True,
                text=True,
            )


class TestPackagePlugins(unittest.TestCase):
    def _write_plugin(
        self,
        plugins_root: Path,
        name: str,
        *,
        wasm: bool = True,
        manifest: bool = True,
    ) -> None:
        plugin_dir = plugins_root / name
        plugin_dir.mkdir(parents=True)
        (plugin_dir / "Cargo.toml").write_text("[package]\nname = \"x\"\n")
        if manifest:
            (plugin_dir / f"{name}.toml").write_text("[plugin]\nid = \"x\"\n")
        if wasm:
            (plugin_dir / f"{name}.wasm").write_bytes(b"\0asm")

    def test_copy_bundled_plugins_copies_required_sidecars(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            plugins_root = root / "plugins-src"
            plugins_dest = root / "pkg" / "plugins"
            plugins_root.mkdir()
            plugins_dest.mkdir(parents=True)
            self._write_plugin(plugins_root, "example-plugin")

            copied = _package.copy_bundled_plugins(plugins_dest, plugins_root)

            self.assertEqual(copied, ["example-plugin"])
            self.assertTrue((plugins_dest / "example-plugin.toml").is_file())
            self.assertTrue((plugins_dest / "example-plugin.wasm").is_file())

    def test_copy_bundled_plugins_skips_unused_plugins(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            plugins_root = root / "plugins-src"
            plugins_dest = root / "pkg" / "plugins"
            plugins_root.mkdir()
            plugins_dest.mkdir(parents=True)
            self._write_plugin(
                plugins_root,
                "gstreamer-preview",
                wasm=False,
                manifest=False,
            )

            copied = _package.copy_bundled_plugins(plugins_dest, plugins_root)

            self.assertEqual(copied, [])
            self.assertEqual(list(plugins_dest.iterdir()), [])

    def test_copy_bundled_plugins_fails_when_wasm_is_missing(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            plugins_root = root / "plugins-src"
            plugins_dest = root / "pkg" / "plugins"
            plugins_root.mkdir()
            plugins_dest.mkdir(parents=True)
            self._write_plugin(plugins_root, "example-plugin", wasm=False)

            with self.assertRaises(SystemExit):
                _package.copy_bundled_plugins(plugins_dest, plugins_root)

    def test_copy_bundled_plugins_fails_when_manifest_is_missing(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            plugins_root = root / "plugins-src"
            plugins_dest = root / "pkg" / "plugins"
            plugins_root.mkdir()
            plugins_dest.mkdir(parents=True)
            self._write_plugin(plugins_root, "example-plugin", manifest=False)

            with self.assertRaises(SystemExit):
                _package.copy_bundled_plugins(plugins_dest, plugins_root)


class TestPackageFreshness(unittest.TestCase):
    def test_stale_binary_fails_when_source_is_newer(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            binary = root / "target" / "release" / "arclain_ui.exe"
            source = root / "crates" / "ui" / "src" / "main.rs"
            binary.parent.mkdir(parents=True)
            source.parent.mkdir(parents=True)
            binary.write_bytes(b"old exe")
            source.write_text("fn main() {}\n")
            os.utime(binary, (100, 100))
            os.utime(source, (200, 200))

            with self.assertRaises(SystemExit):
                _package.ensure_binary_fresh(binary, [source])

    def test_fresh_binary_passes_when_newer_than_sources(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            binary = root / "target" / "release" / "arclain_ui.exe"
            source = root / "crates" / "ui" / "src" / "main.rs"
            binary.parent.mkdir(parents=True)
            source.parent.mkdir(parents=True)
            binary.write_bytes(b"new exe")
            source.write_text("fn main() {}\n")
            os.utime(source, (100, 100))
            os.utime(binary, (200, 200))

            _package.ensure_binary_fresh(binary, [source])


class TestReleaseWorkflows(unittest.TestCase):
    def test_headless_ci_runs_all_non_ui_test_targets(self):
        woodpecker = (REPO_ROOT / ".woodpecker.yml").read_text(encoding="utf-8")
        compose = (REPO_ROOT / "compose.yaml").read_text(encoding="utf-8")
        complete_non_ui = "cargo nextest run --workspace --exclude arclain_ui --locked"
        ui_lib_only = "cargo nextest run -p arclain_ui --lib --locked"
        for text in (woodpecker, compose):
            self.assertIn(complete_non_ui, text)
            self.assertIn(ui_lib_only, text)
        self.assertNotIn("cargo nextest run --workspace --lib --locked", woodpecker)

    def test_release_script_does_not_override_cargo_target_dir(self):
        release_script = (REPO_ROOT / "scripts" / "release.py").read_text(
            encoding="utf-8",
        )

        self.assertNotIn("CARGO_TARGET_DIR", release_script)

    def test_release_command_does_not_run_cargo_tests(self):
        release = importlib.import_module("release")
        woodpecker = (REPO_ROOT / ".woodpecker.yml").read_text(encoding="utf-8")

        self.assertNotIn("cargo\", \"test", inspect.getsource(release.cmd_release))
        self.assertNotIn("release --skip-tests", woodpecker)

    def test_windows_workflow_builds_tests_and_uploads_zip(self):
        workflow = (
            REPO_ROOT / ".github" / "workflows" / "windows-build.yml"
        ).read_text(encoding="utf-8")

        self.assertIn("scripts/_plugins.py build", workflow)
        self.assertIn("scripts/_package.py --profile release --archive", workflow)
        self.assertIn("ARCLAIN_BUNDLED_PLUGIN_DIR", workflow)
        self.assertIn(
            "bundled_dlsite_plugin_loads_against_current_host",
            workflow,
        )
        self.assertIn("arclain-$tag-windows-x64.zip", workflow)
        self.assertNotIn("windows-x64.exe", workflow)

    def test_release_ci_ignores_independent_plugin_tags(self):
        github_workflow = (
            REPO_ROOT / ".github" / "workflows" / "windows-build.yml"
        ).read_text(encoding="utf-8")
        woodpecker = (REPO_ROOT / ".woodpecker.yml").read_text(encoding="utf-8")

        self.assertNotIn("dlsite-metadata-[0-9]*", github_workflow)
        self.assertNotIn("^(dlsite-metadata-)?[0-9]", woodpecker)

    def test_woodpecker_release_tests_packaged_plugins(self):
        woodpecker = (REPO_ROOT / ".woodpecker.yml").read_text(encoding="utf-8")

        self.assertIn("ARCLAIN_BUNDLED_PLUGIN_DIR", woodpecker)
        self.assertIn(
            "bundled_dlsite_plugin_loads_against_current_host",
            woodpecker,
        )


class TestPluginFetchRouting(unittest.TestCase):
    def _source(self, relative_path: str) -> str:
        return (REPO_ROOT / relative_path).read_text(encoding="utf-8")

    def test_manager_registers_manifest_network_capability_and_exact_rpm(self):
        manager = self._source("crates/plugins/src/manager/mod.rs")
        lifecycle = self._source("crates/plugins/src/manager/lifecycle.rs")
        combined = manager + lifecycle

        self.assertIn("PluginNetworkPolicy", combined)
        self.assertIn("configure_plugin", combined)
        self.assertIn("http_requests_per_minute", combined)
        self.assertIn("PluginCapability::Network", combined)

    def test_plugin_data_paths_use_checked_network_entry_points(self):
        resolver = self._source("crates/data/src/features/resolver/network.rs")
        streaming = self._source("crates/data/src/features/streaming_download.rs")
        host = self._source("crates/plugins/src/host_functions/mod.rs")

        self.assertIn("pub fn for_plugin", resolver)
        self.assertIn("blocking_get_for_plugin", resolver)
        self.assertNotIn("should_use_proxy_for_plugin", resolver)
        self.assertIn(".blocking_get(url, false)", resolver)
        self.assertNotIn(".blocking_get(url, use_proxy)", resolver)

        self.assertIn("blocking_get_streaming_for_plugin_with_metadata", streaming)
        self.assertIn("blocking_get_streaming_with_metadata", streaming)
        self.assertNotIn("should_use_proxy_for_plugin", streaming)

        self.assertNotIn("should_use_proxy_for_plugin", host)
        self.assertNotIn("Fall through to the buffered path as a fallback", host)

    def test_plugin_images_are_checked_and_host_images_remain_host_owned(self):
        image_fetcher = self._source("crates/ui/src/shared/image_fetcher.rs")

        self.assertIn("client.request_for_plugin(pid, request)", image_fetcher)
        self.assertIn("Ok(client.request(request))", image_fetcher)


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
