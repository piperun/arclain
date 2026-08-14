from __future__ import annotations

import tempfile
import textwrap
import unittest
from pathlib import Path

from scripts.wirt_dependency import check


WIRT_GIT = "https://codeberg.org/0xdev/wirt.git"
WIRT_REV = "a" * 40
GUEST_LOCKS = (
    "plugins/dlsite-metadata/Cargo.lock",
    "plugins/facade-test-fixture/Cargo.lock",
    "plugins/gstreamer-preview/Cargo.lock",
    "plugins/ui-demo/Cargo.lock",
    "crates/plugins/tests/fixtures/failing-init/Cargo.lock",
    "crates/plugins/tests/fixtures/malicious-metadata/Cargo.lock",
)
GUEST_MANIFESTS = tuple(path.replace("Cargo.lock", "Cargo.toml") for path in GUEST_LOCKS)


class TestWirtDependency(unittest.TestCase):
    def fixture(
        self,
        *,
        root_manifest: str = "members = []",
        manifests: dict[str, str] | None = None,
        directories: list[str] | None = None,
        files: dict[str, str] | None = None,
        toolchain: bool = True,
        locks: bool = True,
        root_dependency: bool = True,
    ) -> Path:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        workspace_dependency = (
            f'\n[workspace.dependencies]\nwirt = {{ git = "{WIRT_GIT}", rev = "{WIRT_REV}" }}\n'
            if root_dependency
            else ""
        )
        self.write(
            root,
            "Cargo.toml",
            f"[workspace]\n{root_manifest}\n{workspace_dependency}",
        )
        if toolchain:
            self.write(
                root,
                "wirt-toolchain.toml",
                textwrap.dedent(
                    f'''\
                    [wirt]
                    git = "{WIRT_GIT}"
                    rev = "{WIRT_REV}"
                    cli_version = "0.3.0"
                    abi = "0.3.0"
                    '''
                ),
            )
        for relative in directories or []:
            (root / relative).mkdir(parents=True, exist_ok=True)
        for relative, content in (manifests or {}).items():
            self.write(root, relative, content)
        for relative, content in (files or {}).items():
            self.write(root, relative, content)
        if locks:
            for lock_relative, manifest_relative in zip(GUEST_LOCKS, GUEST_MANIFESTS):
                self.write(root, lock_relative, self.lock_source(WIRT_REV))
                manifest_path = root / manifest_relative
                if not manifest_path.exists():
                    self.write(
                        root,
                        manifest_relative,
                        textwrap.dedent(
                            f'''\
                            [dependencies]
                            wirt-sdk = {{ git = "{WIRT_GIT}", rev = "{WIRT_REV}" }}
                            '''
                        ),
                    )
        return root

    @staticmethod
    def write(root: Path, relative: str, content: str) -> None:
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")

    @staticmethod
    def lock_source(revision: str, *, url: str = WIRT_GIT) -> str:
        return textwrap.dedent(
            f'''\
            version = 4

            [[package]]
            name = "wirt-sdk"
            version = "0.3.0"
            source = "git+{url}?rev={revision}#{revision}"
            '''
        )

    def test_rejects_embedded_wirt_and_path_dependencies(self):
        root = self.fixture(
            root_manifest='members = ["crates/wirt"]',
            manifests={
                "crates/app/Cargo.toml": (
                    '[dependencies]\nwirt = { path = "../wirt" }\n'
                ),
            },
            directories=["crates/wirt", "wirt-sdk"],
        )
        self.assertEqual(
            check(root),
            [
                "Cargo.toml: embedded Wirt workspace member crates/wirt",
                "crates/app/Cargo.toml: Wirt dependency is not an exact Git revision",
                "crates/wirt: embedded Wirt source remains",
                "wirt-sdk: embedded Wirt SDK remains",
            ],
        )

    def test_rejects_mismatched_host_and_guest_revisions(self):
        other = "b" * 40
        root = self.fixture(
            manifests={
                "crates/app/Cargo.toml": textwrap.dedent(
                    f'''\
                    [dependencies]
                    wirt = {{ git = "{WIRT_GIT}", rev = "{WIRT_REV}" }}
                    '''
                ),
                "plugins/example/Cargo.toml": textwrap.dedent(
                    f'''\
                    [dependencies]
                    wirt-sdk = {{ git = "{WIRT_GIT}", rev = "{other}" }}
                    '''
                ),
            },
        )
        self.assertEqual(
            check(root),
            [
                "plugins/example/Cargo.toml: Wirt dependency is not an exact Git revision",
            ],
        )

    def test_rejects_branch_only_dependency(self):
        root = self.fixture(
            manifests={
                "plugins/example/Cargo.toml": textwrap.dedent(
                    f'''\
                    [dependencies]
                    wirt-sdk = {{ git = "{WIRT_GIT}", branch = "main" }}
                    '''
                ),
            },
        )
        self.assertEqual(
            check(root),
            [
                "plugins/example/Cargo.toml: Wirt dependency is not an exact Git revision",
            ],
        )

    def test_rejects_absent_toolchain_file(self):
        root = self.fixture(toolchain=False)
        self.assertEqual(
            check(root),
            ["wirt-toolchain.toml: Wirt toolchain pin is missing"],
        )

    def test_rejects_a_duplicate_local_wirt_package(self):
        root = self.fixture(
            files={
                "fixtures/plugin.wit": "package wirt:plugin@0.3.0;\n",
            },
        )
        self.assertEqual(
            check(root),
            ["fixtures/plugin.wit: duplicate local Wirt package remains"],
        )

    def test_rejects_source_that_names_the_removed_fixture_tree(self):
        root = self.fixture(
            files={
                "crates/plugins/tests/example.rs": (
                    'const FIXTURE: &str = "crates/'
                    'wirt/tests/fixtures/bundled/ui-demo.wasm";\n'
                ),
            },
        )
        self.assertEqual(
            check(root),
            [
                "crates/plugins/tests/example.rs: removed Wirt fixture path remains",
            ],
        )

    def test_rejects_each_guest_lock_without_the_exact_sdk_source(self):
        for lock_path in GUEST_LOCKS:
            with self.subTest(lock_path=lock_path):
                root = self.fixture()
                self.write(root, lock_path, self.lock_source("b" * 40))
                self.assertEqual(
                    check(root),
                    [f"{lock_path}: wirt-sdk lock source is not the exact Git revision"],
                )

    def test_rejects_missing_guest_lock(self):
        root = self.fixture()
        missing = root / GUEST_LOCKS[-1]
        missing.unlink()
        self.assertEqual(
            check(root),
            [f"{GUEST_LOCKS[-1]}: guest lockfile is missing"],
        )

    def test_rejects_missing_required_root_dependency(self):
        root = self.fixture(root_dependency=False)
        self.assertEqual(
            check(root),
            ["Cargo.toml: required Wirt workspace dependency is missing"],
        )

    def test_rejects_missing_required_guest_dependency(self):
        root = self.fixture()
        self.write(root, GUEST_MANIFESTS[-1], "[dependencies]\n")
        self.assertEqual(
            check(root),
            [
                f"{GUEST_MANIFESTS[-1]}: required wirt-sdk dependency is missing",
            ],
        )

    def test_accepts_one_exact_external_wirt_revision(self):
        root = self.fixture(
            root_manifest='members = ["crates/app"]',
            manifests={
                "crates/app/Cargo.toml": (
                    "[dependencies]\nwirt.workspace = true\n"
                ),
                "plugins/example/Cargo.toml": textwrap.dedent(
                    f'''\
                    [dependencies]
                    wirt-sdk = {{ git = "{WIRT_GIT}", rev = "{WIRT_REV}" }}
                    '''
                ),
            },
        )
        self.assertEqual(check(root), [])


if __name__ == "__main__":
    unittest.main()
