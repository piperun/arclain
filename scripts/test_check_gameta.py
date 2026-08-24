from __future__ import annotations

import unittest

from scripts._check_gameta import gameta_tree_lines


class TestGametaTreeParser(unittest.TestCase):
    def test_matches_package_names_without_matching_the_checkout_path(self):
        tree = "\n".join(
            (
                "arclain_app v2.3.2 (C:/work/filer-wirt-gameta/arclain/crates/app)",
                "├── arclain_core v2.3.2 (C:/work/filer-wirt-gameta/arclain/crates/core)",
                "└── gameta_core v0.5.0 (C:/work/gameta/gameta_core)",
            )
        )

        self.assertEqual(
            gameta_tree_lines(tree),
            ["gameta_core v0.5.0 (C:/work/gameta/gameta_core)"],
        )


if __name__ == "__main__":
    unittest.main()
