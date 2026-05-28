# Arclain dev & release tasks. Run `just --list` to see all recipes.
#
# Cross-platform: works on Windows (PowerShell) and Linux/macOS (sh).
# Heavy logic lives in scripts/_*.py — keeps recipes readable and
# argparse-free dispatch out of the way.

set windows-shell := ["powershell.exe", "-NoProfile", "-Command"]

python := if os_family() == "windows" { "python" } else { "python3" }

# Default when you type bare `just`: fast iteration build.
default: debug

# Fast iteration: debug binary + plugins, no zip, no tests.
debug:
    cargo build -p arclain_ui
    just plugins
    {{python}} scripts/_package.py --profile debug

# `skip-tests=true` skips `cargo test --workspace` (hotfixes only).

# Full release: tests, optimized binary, plugins, zip + sha256.
release skip-tests="false":
    {{ if skip-tests == "true" { "echo 'Skipping tests'" } else { "cargo test --workspace" } }}
    cargo build --release -p arclain_ui
    just plugins
    {{python}} scripts/_package.py --profile release --archive

# Build WASM plugins for all crates under plugins/.
plugins:
    {{python}} scripts/_plugins.py build

# Remove .wasm artifacts and cargo clean each plugin.
clean-plugins:
    {{python}} scripts/_plugins.py clean

# Extra args forward verbatim: `just ui --features dev-foo`.

# Run `cargo ui` with RUST_LOG from scripts/logging_config.json.
ui *args:
    {{python}} scripts/_ui.py {{args}}

# `just deps`                          — outdated check
# `just deps --update`                 — bump Cargo.lock
# `just deps --upgrade`                — bump Cargo.toml (needs cargo-edit)
# `just deps --upgrade --incompatible` — include breaking version bumps
# Add `--dry-run` to any to preview.

# Cargo dependency tools (outdated / update / upgrade).
deps *args:
    {{python}} scripts/_deps.py {{args}}

# Run the Python helper unit + smoke tests (fast).
test-scripts:
    {{python}} -m unittest discover -s scripts -p "test_*.py"

# ─── cocogitto ──────────────────────────────────────────────────────────
# Tag name/prefix is controlled by `cog.toml` (cog 7 default = unprefixed
# version, no `v`). The `-A/--annotated` flag still exists in cog 7 — it
# makes the created tag annotated (tagger + date + message) instead of the
# default lightweight tag. These recipes omit it: arclain's changelog shows
# no 1970-01-01 dates without it, so lightweight tags are fine here. Append
# `-A "msg"` to a bump if you want annotated tags. All bump recipes accept
# extra args, e.g. `just bump --dry-run`.

# Verify commit messages on the current branch.
cog-check:
    cog check

# Generate CHANGELOG.md from conventional commits.
changelog:
    cog changelog

# Auto-detect bump type from conventional commits; create the tag.
bump *args:
    cog bump --auto {{args}}

# Explicit major bump.
bump-major *args:
    cog bump --major {{args}}

# Explicit minor bump.
bump-minor *args:
    cog bump --minor {{args}}

# Explicit patch bump.
bump-patch *args:
    cog bump --patch {{args}}

# Workspace bump using the `release` hook profile (cargo test + check).
bump-release *args:
    cog bump --auto --hook-profile release {{args}}

# Bump a specific monorepo package: `just bump-package dlsite-metadata`.
bump-package name *args:
    cog bump --package {{name}} --auto {{args}}

# Push the latest bump to both remotes.
#
# Branch and tag go in SEPARATE pushes. A combined
# `git push <remote> master --tags` delivers the tag bundled with the
# branch update, and in this setup that does NOT raise Woodpecker's
# `event: tag` pipeline on codeberg — so the Linux build + release
# step never fires. A dedicated push of just the tag produces a clean
# tag-create event that Woodpecker acts on. (Confirmed empirically:
# deleting the tag on codeberg and re-pushing only the tag triggers
# the release.)
#
# We push exactly the tag(s) at HEAD — `$(git tag --points-at HEAD)`,
# i.e. whatever `cog bump` just created — instead of `--tags`. `--tags`
# would shove every local tag at the remote (stale experiments, package
# tags, …), each a potential spurious pipeline; pushing only the bump
# tag keeps the release event clean and intentional. The `$(…)` form
# works in both PowerShell and sh, so the recipe stays cross-platform.
#
# GitHub MUST go first. The codeberg→GitHub push-mirror replicates
# refs but does NOT trigger GitHub Actions on the mirrored push, so
# the Windows build never fires when we only push to codeberg. By
# pushing to GitHub directly first — before codeberg even has the
# tag for the mirror to sync — GitHub sees a real authored push and
# runs the windows-build workflow. Codeberg second drives Woodpecker
# (Linux build + the release the Windows job uploads into).
#
# A tag push only fires CI if the tag is NEW to that remote. If a
# release's tag already landed on a remote (e.g. an earlier combined
# push put it there), the remote won't re-fire — delete + re-push the
# tag to force a fresh event:
#   git push <remote> :refs/tags/<tag>
#   git push <remote> refs/tags/<tag>
push-release:
    git push github master
    git push github $(git tag --points-at HEAD)
    git push origin master
    git push origin $(git tag --points-at HEAD)
