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

# ── just meta ──
# List recipes (projects can do `default: list`).
list:
    @just --list

# Validate this repo's just setup: justfile + vendored lib parse & imports resolve.
test-just:
    @just --summary
