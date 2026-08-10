# Arclain dev & release tasks. Run `just --list` to see all recipes.
#
# Cross-platform: works on Windows (PowerShell) and Linux/macOS (sh).
# Heavy logic lives in scripts/_*.py — keeps recipes readable and
# argparse-free dispatch out of the way.

set windows-shell := ["pwsh", "-NoProfile", "-Command"]
import? 'just/common.just'

# Show available recipes (bare `just`).
default:
    @just --list

# ─── build ────────────────────────────────────────────────────────────────
# `just build` (ui), `just build release`, `just build ui --features foo`.

# Build a target. scope: ui (default) | release | plugins.
build scope="ui" *args:
    just _build-{{scope}} {{args}}

_build-ui *args:
    cargo build -p arclain_ui {{args}}

_build-release *args:
    cargo build --release -p arclain_ui {{args}}

_build-plugins:
    just plugins

# Fast iteration: debug binary + plugins, no zip, no tests.
debug:
    {{python}} scripts/release.py debug

# ─── test ─────────────────────────────────────────────────────────────────
# `just test` (all), `just test rust`, `just test ui -- --nocapture`.

# Run test suites. scope: all (default) | rust | ui | core | plugins | scripts.
test scope="all" *args:
    just _test-{{scope}} {{args}}

_test-all:
    just _test-rust
    just _test-scripts

_test-rust *args:
    cargo test --workspace {{args}}

_test-ui *args:
    cargo test -p arclain_ui {{args}}

_test-core *args:
    cargo test -p arclain_core {{args}}

_test-plugins *args:
    cargo test -p arclain_plugins {{args}}

_test-scripts:
    {{python}} -m unittest discover -s scripts -p "test_*.py"

# ─── formatting ───────────────────────────────────────────────────────────

fmt:
    {{python}} scripts/_format.py

fmt-check:
    {{python}} scripts/_format.py --check

# ─── release ──────────────────────────────────────────────────────────────
# `just release` builds plugins, packages the optimized binary, and archives it.

# Full release: optimized binary, plugins, zip + sha256.
release *args:
    {{python}} scripts/release.py release {{args}}

# ─── plugins ──────────────────────────────────────────────────────────────

# Build validated .wirt archives for all crates under plugins/.
plugins:
    {{python}} scripts/_plugins.py build

# Remove generated plugin archives/components and clean each project.
clean-plugins:
    {{python}} scripts/_plugins.py clean

# Run the repository-owned Wirt developer command.
wirt *args:
    cargo run -p wirt-cli -- {{args}}

# ─── app/dev helpers ──────────────────────────────────────────────────────
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

# ─── cocogitto ──────────────────────────────────────────────────────────
# cog-check / changelog / bump[ major|minor|patch] come from the shared
# library; arclain uses lightweight tags (tag_template stays the default "").
# arclain keeps its workspace- and package-scoped bump variants below.

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

# ─── invariant checks ─────────────────────────────────────────────────────
# `just check` (all), `just check gameta`, `just check boundary`, or
# `just check wirt`.

# Run invariant checks. subject: all (default) | gameta | boundary.
check subject="all":
    just _check-{{subject}}

_check-all:
    just _check-gameta
    just _check-boundary
    just _check-wirt

# The gameta feature's contract: a no-default-features `arclain_app`
# pulls in no gameta crate, that configuration compiles with all targets,
# and the two feature-off test suites the defaults workspace never
# compiles both pass (scripts/_check_gameta.py).
_check-gameta:
    {{python}} scripts/_check_gameta.py

# The headless/GUI crate split: reports Cargo.toml + source-tree
# violations (scripts/frontend_boundary.py). A nonzero exit is expected
# until the app-facade migration removes the direct headless deps it
# flags.
_check-boundary:
    {{python}} scripts/frontend_boundary.py

# Wirt must remain a product-neutral plugin kernel.
_check-wirt:
    {{python}} scripts/wirt_boundary.py

test-frontend-boundary:
    {{python}} scripts/test_frontend_boundary.py

test-wirt-boundary:
    {{python}} scripts/test_wirt_boundary.py
