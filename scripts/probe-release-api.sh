#!/usr/bin/env sh
# probe-release-api.sh — diagnose why publish-release fails for arclain.
#
# Woodpecker's plugin-release step returns the opaque
#   "failed to create release: The target couldn't be found"
# error wrapping whatever Codeberg's Forgejo API actually said. This
# script makes the same API call locally with curl so we see the raw
# response body + headers — the part the plugin swallows.
#
# Usage:
#   CODEBERG_TOKEN=xxxxxxxx ./scripts/probe-release-api.sh
#   CODEBERG_TOKEN=xxxxxxxx ./scripts/probe-release-api.sh --keep
#   CODEBERG_TOKEN=xxxxxxxx ./scripts/probe-release-api.sh --include-real-tag
#
# --keep              Don't delete the probe release/tag created by
#                     synthetic steps. Useful for UI inspection.
# --include-real-tag  Also attempt to create a release for the real
#                     2.2.4 tag. NOT cleaned up — succeeds become a
#                     real (asset-less) release entry. Off by default.
#
# Token scopes required for a full probe:
#   - read:repository (steps 1, 2)
#   - write:repository (step 3 onwards)
# If your token is read-only, step 3 will fail with 403 and that
# itself is the answer.

set -eu

REPO_OWNER="0xdev"
REPO_NAME="Arclain"
BASE="https://codeberg.org/api/v1"
PROBE_TAG="ci-probe-$(date +%s)"
KEEP=0
INCLUDE_REAL=0

for arg in "$@"; do
    case "$arg" in
        --keep) KEEP=1 ;;
        --include-real-tag) INCLUDE_REAL=1 ;;
        -h|--help)
            sed -n '2,24p' "$0"
            exit 0
            ;;
        *) echo "unknown arg: $arg" >&2; exit 2 ;;
    esac
done

if [ -z "${CODEBERG_TOKEN:-}" ]; then
    echo "ERROR: set CODEBERG_TOKEN env var (same secret Woodpecker uses)" >&2
    exit 2
fi

hr() { printf -- '─%.0s' $(seq 1 64); echo; }

# Wrapper: dump status + headers + body for every call.
probe() {
    label=$1
    method=$2
    url=$3
    shift 3
    hr
    echo "[$label] $method $url"
    hr
    # -w writes status code at end; -i includes headers; -sS shows errors
    # but stays quiet on success; no -f so we DON'T mask 4xx/5xx bodies.
    curl -sS -i -X "$method" \
        -H "Authorization: token $CODEBERG_TOKEN" \
        -H "Accept: application/json" \
        "$@" \
        "$url" \
        -w "\n--- exit: %{http_code} ---\n"
    echo
}

# ── Step 1: confirm token can read the repo ────────────────────────────
probe "read repo metadata" GET \
    "$BASE/repos/$REPO_OWNER/$REPO_NAME"

# ── Step 2: confirm token can list releases on this repo ───────────────
probe "list releases" GET \
    "$BASE/repos/$REPO_OWNER/$REPO_NAME/releases?limit=1"

# ── Step 3: REPRODUCE the plugin's POST verbatim ───────────────────────
#
# This mirrors what woodpeckerci/plugin-release sends. We use a synthetic
# probe tag (`ci-probe-<unix>`) so we don't collide with the real 2.2.x
# tags, and target_commitish=master so Forgejo creates the tag at HEAD.
#
# If THIS step returns the "the target couldn't be found" message, we
# know it's a server-side rejection (not anything woodpecker-specific).
# If it succeeds, the bug is on the woodpecker side (env interpolation,
# secret formatting, runner-context).
probe "CREATE probe release (target=master)" POST \
    "$BASE/repos/$REPO_OWNER/$REPO_NAME/releases" \
    -H "Content-Type: application/json" \
    -d "{\"tag_name\":\"$PROBE_TAG\",\"name\":\"$PROBE_TAG\",\"target_commitish\":\"master\",\"draft\":false,\"prerelease\":false}"

# ── Step 3b: same call with no target_commitish (Forgejo defaults to default branch) ─
PROBE_TAG_2="${PROBE_TAG}-no-target"
probe "CREATE probe release (no target)" POST \
    "$BASE/repos/$REPO_OWNER/$REPO_NAME/releases" \
    -H "Content-Type: application/json" \
    -d "{\"tag_name\":\"$PROBE_TAG_2\",\"name\":\"$PROBE_TAG_2\",\"draft\":false,\"prerelease\":false}"

# ── Step 4 (opt-in): try against the existing 2.2.4 tag ────────────────
# 2.2.4 already exists on the remote but has no release. This is the
# closest possible simulation of what woodpecker tried to do — but a
# successful POST here leaves a real (asset-less) release on the public
# release page, so it's gated behind --include-real-tag.
if [ "$INCLUDE_REAL" -eq 1 ]; then
    probe "CREATE release for existing 2.2.4 tag" POST \
        "$BASE/repos/$REPO_OWNER/$REPO_NAME/releases" \
        -H "Content-Type: application/json" \
        -d "{\"tag_name\":\"2.2.4\",\"name\":\"2.2.4\",\"draft\":false,\"prerelease\":false}"
fi

# ── Cleanup (unless --keep) ────────────────────────────────────────────
#
# We delete BOTH the release and the synthetic tag so the probe leaves
# no trace. The 2.2.4 release (if step 4 succeeded) is intentionally
# kept since that's the actual release the project wants.
if [ "$KEEP" -eq 0 ]; then
    echo
    echo "Cleaning up probe releases + tags…"
    for tag in "$PROBE_TAG" "$PROBE_TAG_2"; do
        # Look up release by tag (may 404 if creation failed)
        rid=$(curl -sS -H "Authorization: token $CODEBERG_TOKEN" \
            "$BASE/repos/$REPO_OWNER/$REPO_NAME/releases/tags/$tag" \
            | grep -oE '"id":[0-9]+' | head -1 | cut -d: -f2 || true)
        if [ -n "$rid" ]; then
            echo "  deleting release id=$rid (tag=$tag)"
            curl -sS -o /dev/null -w "    release delete -> %{http_code}\n" \
                -X DELETE -H "Authorization: token $CODEBERG_TOKEN" \
                "$BASE/repos/$REPO_OWNER/$REPO_NAME/releases/$rid"
        fi
        # Delete the tag itself (Forgejo create-release auto-creates it).
        echo "  deleting tag $tag"
        curl -sS -o /dev/null -w "    tag delete -> %{http_code}\n" \
            -X DELETE -H "Authorization: token $CODEBERG_TOKEN" \
            "$BASE/repos/$REPO_OWNER/$REPO_NAME/tags/$tag"
    done
else
    echo "--keep set: probe tags $PROBE_TAG and $PROBE_TAG_2 left in place."
fi

echo
hr
echo "Probe complete. Look at each step's response body:"
echo "  - Step 1/2 200 OK   → token has read scope, repo accessible."
echo "  - Step 3 200/201    → API works. Bug is woodpecker-side (env/secret formatting)."
echo "  - Step 3 422 + 'target' → target_commitish rejected. Compare with step 3b."
echo "  - Step 3b 200/201   → confirms plugin's target field is the culprit."
echo "  - Anything 403      → token missing write:repository scope."
echo "  - Anything 401      → token invalid."
echo "  - Anything 404      → repo path lookup failed (case sensitivity?)."
echo "  - Anything 409      → conflict (release already exists for tag)."
hr
