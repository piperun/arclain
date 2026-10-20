# ─── VENDORED — DO NOT EDIT ───────────────────────────────────────────
# Source of truth: monodev/just/common.just. Change it there, then run
# `python just/deploy.py deploy` to re-sync. Local edits are overwritten.
# ──────────────────────────────────────────────────────────────────────
# Shared just recipe library — vendored into projects; imported as:
#   import? 'just/common.just'
# Canonical: monodev/just/common.just. Do NOT edit the vendored copy; change it
# in monodev and re-run deploy.py.
#
# Shell-neutral by design: every recipe is a single, shell-agnostic command, so
# importing this never conflicts with a project's own `set windows-shell`.
set allow-duplicate-recipes   := true    # a project may shadow any recipe here
set allow-duplicate-variables := true    # …and override the config vars below

# ── config (override in the importing justfile, AFTER the import) ──
# cog -A tag template. "" = lightweight tags. Store the LITERAL cog string:
#   sakka: "{{version}}"   chrysalis/filer: "v{{version}}"
tag_template := ""
python := if os_family() == "windows" { "python" } else { "python3" }
flutter_profile := "packaging/release.json"
_A := if tag_template == "" { "" } else { "-A '" + tag_template + "'" }

# ── cocogitto ──
# Verify commit messages. Args pass through: `just cog-check --from-latest-tag`.
cog-check *args:
    cog check {{args}}

# Generate CHANGELOG.md from conventional commits.
changelog *args:
    cog changelog {{args}}

# Bump version + tag. level: auto (default) | major | minor | patch.
# A leading-dash first token (e.g. `just bump --dry-run`) is a flag on auto,
# so both `just bump minor` and `just bump --dry-run` work.
bump level="auto" *args:
    cog bump {{ if level =~ '^-' { "--auto " + level } else if level =~ '^(auto|major|minor|patch)$' { "--" + level } else { error("bump: level must be auto|major|minor|patch") } }} {{_A}} {{args}}

# ── python helpers ──
# Run the project's scripts/ unit + smoke tests.
test-scripts:
    {{python}} -m unittest discover -s scripts -p "test_*.py"

# Strict type-check Python helpers (Python default; TS repos override).
typecheck:
    uvx basedpyright

# ── excludes ──
# Keep worktree copies out of the language servers and search tools that would
# otherwise index the repo once per checkout. Writes what each tool reads
# (analysis_options.yaml, tsconfig, .gdignore, .vscode, .ignore), detected from
# the repo. `just exclude`, `just exclude worktrees dart`.
exclude preset="worktrees" scope="auto":
    {{python}} just/exclude.py {{preset}} --scope {{scope}}

# ── Flutter desktop releases (opt in with packaging/release.json) ──
# Compile against the matching Flatpak SDK; forwarded paths retain spaces.
[linux]
[positional-arguments]
flatpak *args:
    {{python}} just/flutter.py --profile "{{flutter_profile}}" flatpak "$@"

# Print the runtime refs for an explicit Flatpak SDK installation.
[linux]
flatpak-runtimes:
    {{python}} just/flutter.py --profile "{{flutter_profile}}" sdk-refs

# Build a Windows x64 ZIP, including the Visual C++ runtime DLLs.
[windows]
windows-release:
    {{python}} just/flutter.py --profile "{{flutter_profile}}" windows

# Verify a clean version-tag checkout before any release build.
release-check tag="":
    {{python}} just/release.py --profile "{{flutter_profile}}" check "{{tag}}"

# Report the selected release unit, pinned SDK and version.
release-info:
    {{python}} just/release.py --profile "{{flutter_profile}}" info

# Use optional CODEBERG_READ_TOKEN credentials only during CI builds.
release-auth:
    {{python}} just/release.py --profile "{{flutter_profile}}" auth

release-auth-clean:
    {{python}} just/release.py --profile "{{flutter_profile}}" auth-clean

# Collect one built package with its tag/commit/checksum record.
release-stage target:
    {{python}} just/release.py --profile "{{flutter_profile}}" stage "{{target}}"

# Verify all required packages and write SHA256SUMS and release notes.
release-collect:
    {{python}} just/release.py --profile "{{flutter_profile}}" collect

# Publish the same packages on GitHub and Codeberg (requires scoped tokens).
release-publish:
    {{python}} just/release.py --profile "{{flutter_profile}}" publish

# ── just meta ──
# List recipes (projects can do `default: list`).
list:
    @just --list

# Validate this repo's just setup: justfile + vendored lib parse & imports resolve.
test-just:
    @just --summary

# Test and archive a native Rust unit: just package --unit tool.
[unix]
[script("python3")]
[positional-arguments]
package *args:
    import subprocess, sys
    raise SystemExit(subprocess.call([sys.executable, "just/rust_package.py", *sys.argv[1:]]))

[windows]
[script("python")]
[positional-arguments]
package *args:
    import subprocess, sys
    raise SystemExit(subprocess.call([sys.executable, "just/rust_package.py", *sys.argv[1:]]))

# Publish configured registry packages; use --dry-run for verification only.
[unix]
[script("python3")]
[positional-arguments]
publish *args:
    import subprocess, sys
    raise SystemExit(subprocess.call([sys.executable, "just/publish.py", *sys.argv[1:]]))

[windows]
[script("python")]
[positional-arguments]
publish *args:
    import subprocess, sys
    raise SystemExit(subprocess.call([sys.executable, "just/publish.py", *sys.argv[1:]]))
