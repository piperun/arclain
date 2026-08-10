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

import argparse
import importlib
import inspect
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import _package
import _plugins
import _ui
import _format
import release
from _package import get_platform, workspace_version
from _ui import load_rust_log

REPO_ROOT = Path(__file__).resolve().parents[1]


class TestOwnedFormatting(unittest.TestCase):
    ROOT_COMMAND = [
        "cargo",
        "fmt",
        "--package",
        "arclain_app",
        "--package",
        "arclain_app_fs",
        "--package",
        "arclain_checksum",
        "--package",
        "arclain_cli",
        "--package",
        "arclain_core",
        "--package",
        "arclain_data",
        "--package",
        "arclain_db",
        "--package",
        "arclain-network",
        "--package",
        "arclain_plugins",
        "--package",
        "arclain_signals",
        "--package",
        "arclain_theme",
        "--package",
        "arclain_ui",
        "--package",
        "arclain_widgets",
        "--package",
        "wirt",
    ]
    MANIFEST_COMMANDS = [
        ["cargo", "fmt", "--manifest-path", "wirt-sdk/Cargo.toml"],
        [
            "cargo",
            "fmt",
            "--manifest-path",
            "plugins/dlsite-metadata/Cargo.toml",
        ],
        [
            "cargo",
            "fmt",
            "--manifest-path",
            "plugins/facade-test-fixture/Cargo.toml",
        ],
        [
            "cargo",
            "fmt",
            "--manifest-path",
            "plugins/gstreamer-preview/Cargo.toml",
        ],
        ["cargo", "fmt", "--manifest-path", "plugins/ui-demo/Cargo.toml"],
    ]

    def _expected_commands(self, *, check: bool) -> list[list[str]]:
        commands = [self.ROOT_COMMAND.copy()]
        commands.extend(command.copy() for command in self.MANIFEST_COMMANDS)
        if check:
            for command in commands:
                command.extend(("--", "--check"))
        return commands

    def test_commands_are_exact_in_check_mode(self):
        self.assertEqual(
            _format.commands(check=True),
            self._expected_commands(check=True),
        )

    def test_commands_are_exact_in_write_mode(self):
        self.assertEqual(
            _format.commands(check=False),
            self._expected_commands(check=False),
        )

    def test_write_mode_runs_every_command_from_repo_root(self):
        expected = self._expected_commands(check=False)
        completed = subprocess.CompletedProcess([], 0)
        with mock.patch.object(
            _format.subprocess,
            "run",
            return_value=completed,
        ) as run:
            status = _format.format_owned(check=False)

        self.assertEqual(status, 0)
        self.assertEqual(
            run.call_args_list,
            [mock.call(command, cwd=REPO_ROOT) for command in expected],
        )

    def test_check_mode_runs_every_command_from_repo_root(self):
        expected = self._expected_commands(check=True)
        completed = subprocess.CompletedProcess([], 0)
        with mock.patch.object(
            _format.subprocess,
            "run",
            return_value=completed,
        ) as run:
            status = _format.format_owned(check=True)

        self.assertEqual(status, 0)
        self.assertEqual(
            run.call_args_list,
            [mock.call(command, cwd=REPO_ROOT) for command in expected],
        )

    def test_execution_stops_on_first_nonzero_status(self):
        expected = self._expected_commands(check=True)
        results = (
            subprocess.CompletedProcess(expected[0], 0),
            subprocess.CompletedProcess(expected[1], 23),
            subprocess.CompletedProcess(expected[2], 0),
        )
        with mock.patch.object(
            _format.subprocess,
            "run",
            side_effect=results,
        ) as run:
            status = _format.format_owned(check=True)

        self.assertEqual(status, 23)
        self.assertEqual(
            run.call_args_list,
            [
                mock.call(expected[0], cwd=REPO_ROOT),
                mock.call(expected[1], cwd=REPO_ROOT),
            ],
        )

    def test_justfile_has_exact_format_recipes(self):
        lines = (REPO_ROOT / "justfile").read_text(encoding="utf-8").splitlines()
        fmt_index = lines.index("fmt:")
        self.assertEqual(
            lines[fmt_index:fmt_index + 7],
            [
                "fmt:",
                "    {{python}} scripts/_format.py",
                "",
                "fmt-check:",
                "    {{python}} scripts/_format.py --check",
                "",
                "# ─── release ──────────────────────────────────────────────────────────────",
            ],
        )
        self.assertEqual(lines.count("fmt:"), 1)
        self.assertEqual(lines.count("fmt-check:"), 1)

    def test_woodpecker_checks_format_before_workspace(self):
        lines = (REPO_ROOT / ".woodpecker.yml").read_text(
            encoding="utf-8",
        ).splitlines()
        step_start = lines.index("  cargo-check:")
        step_end = lines.index("  cargo-test:")
        step = lines[step_start:step_end]
        format_command = (
            '      - su runner -c "cd /workspace/codeberg/arclain && '
            'python3 scripts/_format.py --check"'
        )
        cargo_command = (
            '      - su runner -c "cd /workspace/codeberg/arclain && '
            'cargo check --workspace --locked"'
        )

        self.assertEqual(step.count(format_command), 1)
        self.assertEqual(step.count(cargo_command), 1)
        self.assertLess(step.index(format_command), step.index(cargo_command))

    def test_woodpecker_future_incompat_check_uses_rust_1_97(self):
        lines = (REPO_ROOT / ".woodpecker.yml").read_text(
            encoding="utf-8",
        ).splitlines()
        step_start = lines.index("  cargo-check:")
        step_end = lines.index("  cargo-test:")
        step = lines[step_start:step_end]
        images = [line for line in step if line.startswith("    image:")]
        workspace_command = (
            '      - su runner -c "cd /workspace/codeberg/arclain && '
            'cargo check --workspace --locked"'
        )
        future_incompat_command = (
            '      - su runner -c "cd /workspace/codeberg/arclain && '
            "RUSTFLAGS='-Dfuture-incompatible' cargo check "
            '-p arclain_theme -p arclain_widgets --locked"'
        )

        self.assertEqual(images, ["    image: rust:1.97"])
        self.assertEqual(step.count(workspace_command), 1)
        self.assertEqual(step.count(future_incompat_command), 1)
        self.assertLess(
            step.index(workspace_command),
            step.index(future_incompat_command),
        )


class TestGametaPin(unittest.TestCase):
    VERSION = "=0.5.0"
    REVISION = "d0932514ff6277dcef067d8e9dcfe1d5dbfe358b"

    def test_gameta_dependencies_and_ci_checkouts_are_pinned(self):
        manifests = (
            "crates/core/Cargo.toml",
            "crates/db/Cargo.toml",
            "crates/plugins/Cargo.toml",
            "plugins/dlsite-metadata/Cargo.toml",
        )
        for relative_path in manifests:
            with self.subTest(manifest=relative_path):
                with (REPO_ROOT / relative_path).open("rb") as handle:
                    dependencies = tomllib.load(handle)["dependencies"]
                gameta_dependencies = {
                    name: dependency
                    for name, dependency in dependencies.items()
                    if name.startswith("gameta_")
                }
                self.assertTrue(gameta_dependencies, relative_path)
                for name, dependency in gameta_dependencies.items():
                    self.assertEqual(
                        dependency.get("version"),
                        self.VERSION,
                        f"{relative_path}: {name}",
                    )

        workflows = {
            ".woodpecker.yml": "gameta",
            ".github/workflows/tests.yml": "codeberg/gameta",
            ".github/workflows/windows-build.yml": "codeberg/gameta",
            ".github/workflows/flatpak-build.yml": "codeberg/gameta",
        }
        for relative_path, checkout_dir in workflows.items():
            with self.subTest(workflow=relative_path):
                text = (REPO_ROOT / relative_path).read_text(encoding="utf-8")
                self.assertIn(self.REVISION, text)
                self.assertIn(
                    f"git clone --no-checkout "
                    f"https://codeberg.org/0xdev/gameta.git {checkout_dir}",
                    text,
                )
                self.assertIn(
                    f"git -C {checkout_dir} fetch --depth 1 origin "
                    f"{self.REVISION}",
                    text,
                )
                self.assertIn(
                    f"git -C {checkout_dir} checkout --detach FETCH_HEAD",
                    text,
                )
                self.assertNotIn(
                    "git clone --depth 1 https://codeberg.org/0xdev/gameta.git",
                    text,
                )

                if relative_path == ".github/workflows/windows-build.yml":
                    equality_check = (
                        f"if ((git -C {checkout_dir} rev-parse HEAD) -ne "
                        f'"{self.REVISION}")'
                    )
                else:
                    equality_check = (
                        f'test "$(git -C {checkout_dir} rev-parse HEAD)" = '
                        f'"{self.REVISION}"'
                    )
                self.assertIn(equality_check, text)


class TestPluginVersions(unittest.TestCase):
    def test_plugin_manifest_versions_match_cargo(self):
        for plugin in (
            "dlsite-metadata",
            "facade-test-fixture",
            "gstreamer-preview",
            "ui-demo",
        ):
            with self.subTest(plugin=plugin):
                root = REPO_ROOT / "plugins" / plugin
                with (root / "Cargo.toml").open("rb") as handle:
                    cargo = tomllib.load(handle)
                    cargo_version = cargo["package"]["version"]
                with (root / "plugin.toml").open("rb") as handle:
                    manifest_version = tomllib.load(handle)["plugin"]["version"]
                self.assertEqual(manifest_version, cargo_version, plugin)
                self.assertEqual(cargo.get("workspace"), {}, plugin)

    def test_dlsite_debug_dump_does_not_require_wasi_wall_clock(self):
        source = (
            REPO_ROOT / "plugins" / "dlsite-metadata" / "src" / "lib.rs"
        ).read_text(encoding="utf-8")

        self.assertNotIn("SystemTime::now", source)
        self.assertIn('format!("dlsite_blocked_{}.html", product_id)', source)


class TestPluginBuild(unittest.TestCase):
    def test_generated_wirt_archives_are_ignored_distribution_outputs(self):
        ignore_rules = (REPO_ROOT / ".gitignore").read_text(encoding="utf-8").splitlines()
        self.assertIn("*.wirt", ignore_rules)

    def test_missing_wasm_target_is_reported_without_installing_it(self):
        missing = subprocess.CompletedProcess(
            ["rustup", "target", "list", "--installed"],
            0,
            stdout="x86_64-pc-windows-msvc\n",
            stderr="",
        )
        with mock.patch.object(
            _plugins.subprocess,
            "run",
            return_value=missing,
        ) as run:
            self.assertFalse(_plugins.ensure_target())

        run.assert_called_once_with(
            ["rustup", "target", "list", "--installed"],
            capture_output=True,
            text=True,
        )

    def test_build_creates_and_validates_exact_wirt_archive_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plugins_dir = root / "plugins"
            plugin_dir = plugins_dir / "example-plugin"
            plugin_dir.mkdir(parents=True)
            (plugin_dir / "Cargo.toml").write_text(
                '[package]\nname = "example-plugin"\nversion = "1.2.3"\n',
                encoding="utf-8",
            )
            (plugin_dir / "plugin.toml").write_text(
                '[plugin]\nid = "example-plugin"\nversion = "1.2.3"\n',
                encoding="utf-8",
            )
            stale_wasm = plugin_dir / "example-plugin.wasm"
            stale_wasm.write_bytes(b"stale raw component")
            expected_package = plugin_dir / "example-plugin-1.2.3.wirt"

            installed = subprocess.CompletedProcess(
                ["rustup", "target", "list", "--installed"],
                0,
                stdout=f"{_plugins.WASM_TARGET}\n",
                stderr="",
            )

            def run_command(command, **_kwargs):
                if command == ["rustup", "target", "list", "--installed"]:
                    return installed
                if command[:4] == ["cargo", "run", "-p", "wirt-cli"]:
                    action = command[5]
                    if action == "package":
                        expected_package.write_bytes(b"deterministic wirt package")
                return subprocess.CompletedProcess(command, 0, "", "")

            with mock.patch.object(_plugins, "PLUGINS_DIR", plugins_dir), \
                 mock.patch.object(_plugins, "REPO_ROOT", root), \
                 mock.patch.object(
                     _plugins.subprocess,
                     "run",
                     side_effect=run_command,
                 ) as run:
                status = _plugins.build()

            self.assertEqual(status, 0)
            self.assertEqual(expected_package.read_bytes(), b"deterministic wirt package")
            self.assertFalse(stale_wasm.exists())
            self.assertEqual(
                [path.name for path in plugin_dir.glob("*.wirt")],
                ["example-plugin-1.2.3.wirt"],
            )
            self.assertEqual(
                [call.args[0][5] for call in run.call_args_list[1:]],
                ["build", "package", "validate"],
            )

    def test_build_fails_when_wirt_package_is_missing_after_command(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plugins_dir = root / "plugins"
            plugin_dir = plugins_dir / "example-plugin"
            plugin_dir.mkdir(parents=True)
            (plugin_dir / "Cargo.toml").write_text(
                '[package]\nname = "example-plugin"\nversion = "1.2.3"\n',
                encoding="utf-8",
            )
            (plugin_dir / "plugin.toml").write_text(
                '[plugin]\nid = "example-plugin"\nversion = "1.2.3"\n',
                encoding="utf-8",
            )
            installed = subprocess.CompletedProcess(
                [], 0, f"{_plugins.WASM_TARGET}\n", "",
            )

            def run_command(command, **_kwargs):
                if command == ["rustup", "target", "list", "--installed"]:
                    return installed
                return subprocess.CompletedProcess(command, 0, "", "")

            with mock.patch.object(_plugins, "PLUGINS_DIR", plugins_dir), \
                 mock.patch.object(_plugins, "REPO_ROOT", root), \
                 mock.patch.object(
                     _plugins.subprocess,
                     "run",
                     side_effect=run_command,
                 ):
                self.assertEqual(_plugins.build(), 1)


class TestPluginClean(unittest.TestCase):
    def test_clean_returns_failure_after_cleaning_every_plugin(self):
        with tempfile.TemporaryDirectory() as directory:
            plugins_dir = Path(directory) / "plugins"
            first_plugin = plugins_dir / "first-plugin"
            second_plugin = plugins_dir / "second-plugin"
            for plugin_dir in (first_plugin, second_plugin):
                plugin_dir.mkdir(parents=True)
                (plugin_dir / "Cargo.toml").write_text("[package]\n")
            (first_plugin / "first-plugin.wasm").write_bytes(b"wasm")
            (first_plugin / "first-plugin-1.0.0.wirt").write_bytes(b"package")

            clean_results = [
                subprocess.CompletedProcess([], 17),
                subprocess.CompletedProcess([], 0),
            ]
            with mock.patch.object(_plugins, "PLUGINS_DIR", plugins_dir), \
                 mock.patch.object(
                     _plugins.subprocess,
                     "run",
                     side_effect=clean_results,
                 ) as run:
                clean_status = _plugins.clean()

            self.assertEqual(clean_status, 1, f"clean_status={clean_status}")
            self.assertFalse((first_plugin / "first-plugin.wasm").exists())
            self.assertFalse((first_plugin / "first-plugin-1.0.0.wirt").exists())
            self.assertEqual(run.call_count, 2)
            self.assertEqual(run.call_args_list[0].kwargs["cwd"], first_plugin)
            self.assertEqual(run.call_args_list[1].kwargs["cwd"], second_plugin)


class TestReleaseDelegation(unittest.TestCase):
    def test_release_delegates_plugin_build_and_packaging_once(self):
        args = argparse.Namespace(skip_tests=True)

        with mock.patch.object(release, "run") as run, \
             mock.patch.object(
                 release._package,
                 "cargo_target_dir",
                 return_value=REPO_ROOT / "target",
             ), \
             mock.patch.object(
                 release._package,
                 "workspace_version",
                 return_value="1.2.3",
             ) as workspace_version, \
             mock.patch.object(_plugins, "build", return_value=0) as build, \
             mock.patch.object(_package, "package") as package:
            release.cmd_release(args)

        run.assert_called_once_with(
            ["cargo", "build", "--release", "--package", "arclain_ui"],
            cwd=REPO_ROOT,
        )
        build.assert_called_once_with()
        workspace_version.assert_called_once_with()
        package.assert_called_once_with(
            profile="release",
            archive=True,
            version="1.2.3",
        )

    def test_debug_delegates_plugin_build_and_packaging_once(self):
        with mock.patch.object(release, "run") as run, \
             mock.patch.object(
                 release._package,
                 "cargo_target_dir",
                 return_value=REPO_ROOT / "target",
             ), \
             mock.patch.object(
                 release._package,
                 "workspace_version",
                 return_value="1.2.3",
             ) as workspace_version, \
             mock.patch.object(_plugins, "build", return_value=0) as build, \
             mock.patch.object(_package, "package") as package:
            release.cmd_debug(argparse.Namespace())

        run.assert_called_once_with(
            ["cargo", "build", "--package", "arclain_ui"],
            cwd=REPO_ROOT,
        )
        build.assert_called_once_with()
        workspace_version.assert_called_once_with()
        package.assert_called_once_with(
            profile="debug",
            archive=False,
            version="1.2.3",
        )

    def test_plugin_commands_delegate_to_canonical_helpers(self):
        with mock.patch.object(_plugins, "build", return_value=0) as build:
            release.cmd_plugins(argparse.Namespace())

        build.assert_called_once_with()

        with mock.patch.object(_plugins, "clean", return_value=0) as clean:
            release.cmd_clean_plugins(argparse.Namespace())

        clean.assert_called_once_with()

    def test_plugin_build_failures_propagate_without_packaging(self):
        commands = (
            (release.cmd_release, argparse.Namespace(skip_tests=False)),
            (release.cmd_debug, argparse.Namespace()),
            (release.cmd_plugins, argparse.Namespace()),
        )
        for command, args in commands:
            with self.subTest(command=command.__name__), \
                 mock.patch.object(release, "run"), \
                 mock.patch.object(
                     release._package,
                     "cargo_target_dir",
                     return_value=REPO_ROOT / "target",
                 ), \
                 mock.patch.object(
                     release._package,
                     "workspace_version",
                     return_value="1.2.3",
                 ), \
                 mock.patch.object(_plugins, "build", return_value=23), \
                 mock.patch.object(_package, "package") as package:
                with self.assertRaises(SystemExit) as raised:
                    command(args)

                self.assertEqual(raised.exception.code, 23)
                package.assert_not_called()

    def test_plugin_clean_failure_propagates_exact_status(self):
        with mock.patch.object(_plugins, "clean", return_value=29), \
             self.assertRaises(SystemExit) as raised:
            release.cmd_clean_plugins(argparse.Namespace())

        self.assertEqual(raised.exception.code, 29)

    def test_release_source_has_no_duplicate_helper_implementations(self):
        source = inspect.getsource(release)

        for helper in (
            "build_plugins",
            "clean_plugins",
            "get_platform",
            "sha256_file",
            "get_version_from_cargo",
        ):
            with self.subTest(helper=helper):
                self.assertNotIn(f"def {helper}(", source)


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
        package: bool = True,
    ) -> None:
        plugin_dir = plugins_root / name
        plugin_dir.mkdir(parents=True)
        (plugin_dir / "Cargo.toml").write_text("[package]\nname = \"x\"\n")
        if package:
            (plugin_dir / f"{name}-1.2.3.wirt").write_bytes(b"wirt package")

    def test_copy_bundled_plugins_copies_validated_wirt_archive_only(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            plugins_root = root / "plugins-src"
            plugins_dest = root / "pkg" / "plugins"
            plugins_root.mkdir()
            plugins_dest.mkdir(parents=True)
            self._write_plugin(plugins_root, "example-plugin")

            with mock.patch.object(
                _package._plugins,
                "validate_package",
                return_value=True,
            ) as validate:
                copied = _package.copy_bundled_plugins(plugins_dest, plugins_root)

            self.assertEqual(copied, ["example-plugin"])
            package = plugins_dest / "example-plugin-1.2.3.wirt"
            self.assertEqual(package.read_bytes(), b"wirt package")
            self.assertEqual(list(plugins_dest.iterdir()), [package])
            validate.assert_called_once_with(
                plugins_root / "example-plugin" / "example-plugin-1.2.3.wirt",
            )

    def test_copy_bundled_plugins_skips_unused_plugins(self):
        # Iterates the *real* `_package.SKIP_PLUGINS` set rather than a
        # hardcoded duplicate: every plugin ever added to that set --
        # including "facade-test-fixture", arclain_app's own test-only
        # fixture -- must be skipped even with no sidecar files at all, so
        # `just release`/`just debug` never depends on (or bundles) it.
        for plugin_name in sorted(_package.SKIP_PLUGINS):
            with self.subTest(plugin=plugin_name):
                with tempfile.TemporaryDirectory() as d:
                    root = Path(d)
                    plugins_root = root / "plugins-src"
                    plugins_dest = root / "pkg" / "plugins"
                    plugins_root.mkdir()
                    plugins_dest.mkdir(parents=True)
                    self._write_plugin(
                        plugins_root,
                        plugin_name,
                        package=False,
                    )

                    copied = _package.copy_bundled_plugins(plugins_dest, plugins_root)

                    self.assertEqual(copied, [])
                    self.assertEqual(list(plugins_dest.iterdir()), [])

    def test_facade_test_fixture_is_never_bundled_into_a_release_package(self):
        # Named regression guard (on top of the parametrized test above):
        # this must keep failing loudly if a future edit ever drops
        # "facade-test-fixture" from `SKIP_PLUGINS`, since the fixture's
        # own "Trigger Trap" button has no business in a user-facing package.
        self.assertIn("facade-test-fixture", _package.SKIP_PLUGINS)

    def test_copy_bundled_plugins_fails_when_wirt_package_is_missing(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            plugins_root = root / "plugins-src"
            plugins_dest = root / "pkg" / "plugins"
            plugins_root.mkdir()
            plugins_dest.mkdir(parents=True)
            self._write_plugin(plugins_root, "example-plugin", package=False)

            with self.assertRaises(SystemExit):
                _package.copy_bundled_plugins(plugins_dest, plugins_root)

    def test_copy_bundled_plugins_fails_when_wirt_package_is_invalid(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            plugins_root = root / "plugins-src"
            plugins_dest = root / "pkg" / "plugins"
            plugins_root.mkdir()
            plugins_dest.mkdir(parents=True)
            self._write_plugin(plugins_root, "example-plugin")

            with mock.patch.object(
                _package._plugins,
                "validate_package",
                return_value=False,
            ), self.assertRaises(SystemExit):
                _package.copy_bundled_plugins(plugins_dest, plugins_root)

            self.assertEqual(list(plugins_dest.iterdir()), [])


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


class TestPackageArchive(unittest.TestCase):
    def test_archive_contains_one_package_root_without_nested_outputs(self):
        with tempfile.TemporaryDirectory() as directory:
            out_root = Path(directory) / "release"
            package_name = "arclain-1.2.3-linux-x64"
            package_dir = out_root / package_name
            package_dir.mkdir(parents=True)
            (package_dir / "arclain").write_bytes(b"binary")

            archive_path, checksum_path = _package.create_archive(
                package_dir,
                out_root,
                "linux",
            )

            self.assertEqual(
                archive_path,
                out_root / f"{package_name}.tar.gz",
            )
            self.assertEqual(
                checksum_path,
                out_root / f"{package_name}.tar.gz.sha256",
            )
            with tarfile.open(archive_path, "r:gz") as archive:
                names = archive.getnames()

            self.assertEqual(names.count(package_name), 1)
            for name in names:
                self.assertTrue(
                    name == package_name or name.startswith(f"{package_name}/"),
                    name,
                )
                self.assertFalse(
                    name.endswith((".tar.gz", ".zip", ".sha256")),
                    name,
                )

    def test_archive_replaces_only_its_exact_outputs(self):
        with tempfile.TemporaryDirectory() as directory:
            out_root = Path(directory) / "release"
            package_name = "arclain-1.2.3-linux-x64"
            package_dir = out_root / package_name
            package_dir.mkdir(parents=True)
            (package_dir / "arclain").write_bytes(b"binary")

            target_archive = out_root / f"{package_name}.tar.gz"
            target_checksum = out_root / f"{package_name}.tar.gz.sha256"
            target_archive.write_bytes(b"stale archive")
            target_checksum.write_text("stale checksum\n", encoding="utf-8")
            unrelated_archive = out_root / "arclain-9.9.9-linux-x64.tar.gz"
            unrelated_checksum = unrelated_archive.with_suffix(
                unrelated_archive.suffix + ".sha256",
            )
            unrelated_archive.write_bytes(b"keep archive")
            unrelated_checksum.write_text("keep checksum\n", encoding="utf-8")

            archive_path, checksum_path = _package.create_archive(
                package_dir,
                out_root,
                "linux",
            )

            self.assertEqual(archive_path, target_archive)
            self.assertEqual(checksum_path, target_checksum)
            self.assertNotEqual(archive_path.read_bytes(), b"stale archive")
            self.assertNotEqual(
                checksum_path.read_text(encoding="utf-8"),
                "stale checksum\n",
            )
            self.assertEqual(unrelated_archive.read_bytes(), b"keep archive")
            self.assertEqual(
                unrelated_checksum.read_text(encoding="utf-8"),
                "keep checksum\n",
            )


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

    def test_woodpecker_publishes_package_helper_archive_once(self):
        text = (REPO_ROOT / ".woodpecker.yml").read_text(encoding="utf-8")

        self.assertNotIn("tar -czf", text)
        self.assertIn("release/arclain-*-linux-x64.tar.gz", text)
        self.assertIn("release/arclain-*-linux-x64.tar.gz.sha256", text)


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
        self.assertIn(".blocking_get_with_limit(url, false, limit)", resolver)
        self.assertNotIn(".blocking_get(url, use_proxy)", resolver)
        self.assertNotIn(".blocking_get_with_limit(url, use_proxy", resolver)

        self.assertIn("blocking_get_streaming_for_plugin_with_metadata", streaming)
        self.assertIn("blocking_get_streaming_with_metadata", streaming)
        self.assertNotIn("should_use_proxy_for_plugin", streaming)

        self.assertNotIn("should_use_proxy_for_plugin", host)
        self.assertNotIn("Fall through to the buffered path as a fallback", host)

    def test_ui_plugin_images_stay_behind_the_application_dependency_boundary(self):
        """Rust tests cover routing behavior; this gate enforces ownership."""
        with (REPO_ROOT / "crates" / "ui" / "Cargo.toml").open("rb") as handle:
            manifest = tomllib.load(handle)

        dependency_tables = [
            manifest.get("dependencies", {}),
            manifest.get("dev-dependencies", {}),
            manifest.get("build-dependencies", {}),
        ]
        for target in manifest.get("target", {}).values():
            dependency_tables.extend(
                [
                    target.get("dependencies", {}),
                    target.get("dev-dependencies", {}),
                    target.get("build-dependencies", {}),
                ]
            )

        def package_names(dependencies):
            return {
                spec.get("package", name) if isinstance(spec, dict) else name
                for name, spec in dependencies.items()
            }

        direct_packages = set().union(
            *(package_names(dependencies) for dependencies in dependency_tables)
        )
        self.assertIn("arclain_app", direct_packages)
        self.assertNotIn("arclain-network", direct_packages)


class TestWirtAbi(unittest.TestCase):
    def test_one_versioned_wit_source_and_no_arclain_namespace(self):
        canonical = REPO_ROOT / "wirt-sdk" / "wit" / "plugin.wit"
        legacy = REPO_ROOT / "wit" / "arclain.wit"
        sdk = (REPO_ROOT / "wirt-sdk" / "src" / "lib.rs").read_text(
            encoding="utf-8",
        )
        self.assertTrue(canonical.is_file())
        self.assertFalse(legacy.exists())
        self.assertEqual(
            canonical.read_text(encoding="utf-8").splitlines()[0],
            "package wirt:plugin@0.1.0;",
        )
        self.assertRegex(
            sdk,
            r'path\s*:\s*"wit/plugin\.wit"',
        )

        roots = (
            REPO_ROOT / "crates" / "plugins" / "src",
            REPO_ROOT
            / "crates"
            / "plugins"
            / "tests"
            / "fixtures"
            / "malicious-metadata"
            / "src",
            REPO_ROOT / "wirt-sdk" / "src",
            REPO_ROOT / "plugins" / "dlsite-metadata" / "src",
            REPO_ROOT / "plugins" / "facade-test-fixture" / "src",
            REPO_ROOT / "plugins" / "gstreamer-preview" / "src",
            REPO_ROOT / "plugins" / "ui-demo" / "src",
        )
        offenders = []
        for root in roots:
            for path in root.rglob("*.rs"):
                text = path.read_text(encoding="utf-8")
                if any(
                    marker in text
                    for marker in (
                        "archust_plugin_sdk::arclain",
                        "arclain::plugin",
                        "arclain.wit",
                    )
                ):
                    offenders.append(path.relative_to(REPO_ROOT).as_posix())
        self.assertEqual(offenders, [])


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
