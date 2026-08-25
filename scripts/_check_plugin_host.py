#!/usr/bin/env python3
"""Enforce the optional plugin-host dependency boundary."""
from __future__ import annotations

import re
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_TREE = ["cargo", "tree", "-p", "arclain_app", "-e", "normal"]
ARCHIVE_ONLY_TREE = [
    "cargo",
    "tree",
    "-p",
    "arclain_app",
    "-e",
    "normal",
    "--no-default-features",
]
ARCHIVE_ONLY_RUSTDOC = [
    "cargo",
    "rustdoc",
    "--locked",
    "-p",
    "arclain_app",
    "--no-default-features",
    "--lib",
    "--",
    "-A",
    "warnings",
    "-D",
    "rustdoc::broken-intra-doc-links",
]
ARCHIVE_ONLY_BEHAVIOR_CHECKS = (
    [
        "cargo",
        "check",
        "--locked",
        "-p",
        "arclain_app",
        "--no-default-features",
        "--lib",
    ],
    [
        "cargo",
        "test",
        "--locked",
        "-p",
        "arclain_app",
        "--no-default-features",
        "--test",
        "plugin_host_disabled",
    ],
)
FORBIDDEN = ("arclain_plugins", "wirt", "wasmtime")
TREE_GLYPHS = "│├└─ \t"
COMPILE_CONTRACT_DIR = REPO_ROOT / "crates/app/tests/compile_contract"
COMPILE_CONTRACTS = (
    ("default_plugin_host.rs", True, True),
    ("archive_only_facade.rs", False, True),
    ("archive_only_rejects_plugin_host.rs", False, False),
)
PLUGIN_FACADE_METHODS = (
    "plugins",
    "set_plugin_enabled",
    "uninstall_plugin",
    "retry_plugin",
    "reset_plugin_quarantine",
    "plugin_settings",
    "set_plugin_settings",
    "open_plugin_session",
    "open_plugin_session_for_archive",
    "plugin_session_archive_origin",
    "plugin_ui_document",
    "close_plugin_session",
    "start_plugin_action",
    "set_active_archive_session",
    "install_active_tab_bridge",
    "active_tab_bridge",
    "read_plugin_image",
    "write_plugin_image",
    "fetch_plugin_image",
    "plugin_domain_whitelist",
    "set_plugin_domain_approved",
    "inspect_plugin_package",
    "install_plugin_package",
    "plugin_chrome",
    "plugin_network_log",
)


def tree_contract_errors(default_tree: str, archive_only_tree: str) -> list[str]:
    """Return dependency-boundary errors for two rendered Cargo trees."""
    errors: list[str] = []
    default_packages = cargo_tree_package_names(default_tree)
    archive_only_packages = cargo_tree_package_names(archive_only_tree)
    for package in FORBIDDEN:
        if package not in default_packages:
            errors.append(f"default tree is missing {package}")
        if package in archive_only_packages:
            errors.append(f"archive-only tree contains {package}")
    return errors


def cargo_tree_package_names(tree: str) -> set[str]:
    """Parse exact package-name tokens from rendered `cargo tree` lines."""
    packages: set[str] = set()
    for line in tree.splitlines():
        crate_line = line.lstrip(TREE_GLYPHS)
        if crate_line:
            packages.add(crate_line.split(maxsplit=1)[0])
    return packages


def plugin_facade_gate_errors(
    source: str, methods: tuple[str, ...] = PLUGIN_FACADE_METHODS
) -> list[str]:
    """Reject known plugin-facing methods without an adjacent cfg gate."""
    errors: list[str] = []
    for method in methods:
        signature = re.compile(
            rf"(?m)^\s*pub\s+(?:async\s+)?fn\s+{re.escape(method)}\b"
        )
        matches = list(signature.finditer(source))
        if not matches:
            errors.append(f"plugin facade method `{method}` is missing")
            continue
        for match in matches:
            preceding = source[: match.start()].rstrip().splitlines()
            if not preceding or preceding[-1].strip() != '#[cfg(feature = "plugin-host")]':
                errors.append(f"plugin facade method `{method}` is not feature-gated")
    return errors


def plugin_host_rustdoc_errors(diagnostics: str) -> list[str]:
    """Return broken rustdoc links whose targets vanish without plugin-host."""
    return [
        line
        for line in diagnostics.splitlines()
        if line.startswith(("error: ", "warning: "))
        and "link" in line
        and (
            "`crate::analyze_url`" in line
            or "`crate::plugins" in line
        )
    ]


def archive_only_rustdoc_errors() -> list[str]:
    """Run the focused rustdoc lane while tolerating unrelated baseline links."""
    result = subprocess.run(
        ARCHIVE_ONLY_RUSTDOC,
        cwd=REPO_ROOT,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
    )
    diagnostics = "\n".join((result.stdout, result.stderr))
    errors = plugin_host_rustdoc_errors(diagnostics)
    if result.returncode != 0 and "could not document `arclain_app`" not in diagnostics:
        errors.append("archive-only rustdoc did not reach its documentation diagnostics")
        sys.stderr.write(diagnostics)
    return errors


def cargo_tree(command: list[str]) -> str | None:
    result = subprocess.run(
        command,
        cwd=REPO_ROOT,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        print(f"check-plugin-host: `{' '.join(command)}` failed", file=sys.stderr)
        return None
    return result.stdout


def archive_only_behavior_errors() -> list[str]:
    """Run the archive-only library and real behavior contracts."""
    errors: list[str] = []
    for command in ARCHIVE_ONLY_BEHAVIOR_CHECKS:
        result = subprocess.run(command, cwd=REPO_ROOT)
        if result.returncode != 0:
            errors.append(f"archive-only check `{' '.join(command)}` failed")
    return errors


def compile_contract_errors() -> list[str]:
    """Compile the public facade in its default and archive-only shapes."""
    errors: list[str] = []
    app_path = (REPO_ROOT / "crates/app").as_posix()
    with tempfile.TemporaryDirectory(prefix="arclain-plugin-host-contract-") as raw:
        project = Path(raw)
        source_dir = project / "src"
        source_dir.mkdir()
        manifest_path = project / "Cargo.toml"
        target_dir = REPO_ROOT / "target"

        for fixture_name, default_features, should_compile in COMPILE_CONTRACTS:
            manifest_path.write_text(
                "\n".join(
                    (
                        "[package]",
                        'name = "arclain-plugin-host-contract"',
                        'version = "0.0.0"',
                        'edition = "2021"',
                        "",
                        "[dependencies]",
                        (
                            f'arclain_app = {{ path = "{app_path}" }}'
                            if default_features
                            else f'arclain_app = {{ path = "{app_path}", default-features = false }}'
                        ),
                        "",
                        "[workspace]",
                        "",
                    )
                ),
                encoding="utf-8",
            )
            (source_dir / "main.rs").write_text(
                (COMPILE_CONTRACT_DIR / fixture_name).read_text(encoding="utf-8"),
                encoding="utf-8",
            )
            command = [
                "cargo",
                "check",
                "--quiet",
                "--manifest-path",
                str(manifest_path),
                "--target-dir",
                str(target_dir),
            ]
            result = subprocess.run(
                command,
                cwd=REPO_ROOT,
                capture_output=True,
                encoding="utf-8",
                errors="replace",
            )
            if should_compile:
                if result.returncode != 0:
                    errors.append(f"{fixture_name} did not compile")
                    sys.stderr.write(result.stderr)
                continue

            expected = (
                "unresolved imports `arclain_app::analyze_url`, "
                "`arclain_app::plugins`"
            )
            if result.returncode == 0:
                errors.append(f"{fixture_name} unexpectedly compiled")
            elif expected not in result.stderr:
                errors.append(f"{fixture_name} failed for the wrong reason")
                sys.stderr.write(result.stderr)

    return errors


def main() -> int:
    default_tree = cargo_tree(DEFAULT_TREE)
    if default_tree is None:
        return 1
    archive_only_tree = cargo_tree(ARCHIVE_ONLY_TREE)
    if archive_only_tree is None:
        return 1

    errors = tree_contract_errors(default_tree, archive_only_tree)
    runtime_source = (REPO_ROOT / "crates/app/src/runtime/mod.rs").read_text(
        encoding="utf-8"
    )
    errors.extend(plugin_facade_gate_errors(runtime_source))
    errors.extend(compile_contract_errors())
    errors.extend(archive_only_rustdoc_errors())
    errors.extend(archive_only_behavior_errors())
    if errors:
        print("Plugin-host dependency boundary violations:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    print("Plugin-host dependency boundary: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
