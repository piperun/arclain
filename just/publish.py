"""Publish explicitly selected packages to a configured registry after native dry runs."""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys

if __name__ == "__main__":
    sys.dont_write_bytecode = True

from release_core import ROOT, project_path
import registry
import rust_publish
import pub_publish


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", default="packaging/release.json")
    parser.add_argument("--registry", choices=registry.REGISTRIES)
    parser.add_argument("--package", action="append", default=[])
    parser.add_argument("--all", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--non-interactive", action="store_true")
    args = parser.parse_args(argv)
    try:
        profile = json.loads(project_path(ROOT, args.profile).read_text(encoding="utf-8"))
        names = registry.configured(profile)
        ci = os.environ.get("CI", "").lower() not in ("", "false", "0", "no")
        interactive = sys.stdin.isatty() and sys.stdout.isatty() and not ci and not args.non_interactive
        selected = args.registry
        if not selected and len(names) == 1:
            selected = names[0]
        elif not selected and interactive and names:
            for index, name in enumerate(names, 1):
                print(f"  {index}) {name}")
            answer = input("Registry: ").strip()
            selected = names[int(answer) - 1] if answer.isdecimal() and 1 <= int(answer) <= len(names) else answer
        if selected not in names:
            raise ValueError("Choose a configured --registry (crates-io or pub.dev); run just setup release to configure outputs")
        options = dict(dry_run=args.dry_run, all_packages=args.all, interactive=interactive)
        if selected == "crates-io":
            rust_publish.publish(ROOT, registry.cargo_profile(ROOT, profile), args.package, **options)
        else:
            pub_publish.publish(ROOT, profile["registries"]["pub.dev"], args.package, **options)
        return 0
    except (EOFError, KeyboardInterrupt):
        print("Publishing cancelled.", file=sys.stderr)
        return 130
    except (ValueError, OSError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        print(f"Registry publishing failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
