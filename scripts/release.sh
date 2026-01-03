#!/usr/bin/env bash
# Arclain Release Build Script (Linux)
# Works both containerized and standalone

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$REPO_ROOT"

echo "=== Arclain Release Build (Linux) ==="
echo "Repository: $REPO_ROOT"
echo ""

# Parse arguments
SKIP_VERSION_UPDATE=false
SKIP_TESTS=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-version-update)
            SKIP_VERSION_UPDATE=true
            shift
            ;;
        --skip-tests)
            SKIP_TESTS=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--skip-version-update] [--skip-tests]"
            exit 1
            ;;
    esac
done

# Check if CARGO_HOME is on tmpfs
TARGET_DIR="$REPO_ROOT/target"
if [ -n "$CARGO_HOME" ]; then
    # Check if CARGO_HOME mount point is tmpfs
    if df -T "$CARGO_HOME" 2>/dev/null | grep -q tmpfs; then
        echo "⚠ CARGO_HOME is on tmpfs, using persistent target directory..."
        TARGET_DIR="$REPO_ROOT/release-target"
        export CARGO_TARGET_DIR="$TARGET_DIR"
    fi
fi

# Detect container environment
IN_CONTAINER=false
if [ -f /.dockerenv ] || grep -q docker /proc/1/cgroup 2>/dev/null; then
    IN_CONTAINER=true
    echo "Running in container environment"
fi

# Step 1: Update versions (skip in container unless explicitly requested)
if [ "$SKIP_VERSION_UPDATE" = false ]; then
    echo ""
    echo "Step 1: Updating crate versions..."
    if [ -f "$SCRIPT_DIR/calculate-versions.sh" ]; then
        bash "$SCRIPT_DIR/calculate-versions.sh" --update-cargo
    elif command -v pwsh &> /dev/null; then
        pwsh "$SCRIPT_DIR/calculate-versions.ps1" -UpdateCargo
    else
        echo "⚠ No version update script available, skipping..."
    fi
else
    echo ""
    echo "Step 1: Skipping version update"
fi

# Read version from arclain_ui
VERSION="0.0.0"
if [ -f "$REPO_ROOT/crates/ui/Cargo.toml" ]; then
    VERSION=$(grep -m1 '^version' "$REPO_ROOT/crates/ui/Cargo.toml" | sed 's/.*"\([^"]*\)".*/\1/')
fi
echo "Building version: $VERSION"

# Step 2: Run tests
if [ "$SKIP_TESTS" = false ]; then
    echo ""
    echo "Step 2: Running test suite..."
    cargo test --workspace
    echo "✓ All tests passed!"
else
    echo ""
    echo "Step 2: Skipping tests"
fi

# Step 3: Build release
echo ""
echo "Step 3: Building release binary..."
cargo build --release --package arclain_ui

# Build plugins
echo "Building plugins..."
if [ -f "$SCRIPT_DIR/build-plugins.sh" ]; then
    bash "$SCRIPT_DIR/build-plugins.sh" || echo "⚠ Plugin build had issues, continuing..."
fi

# Step 4: Package
echo ""
echo "Step 4: Packaging release..."

# Detect architecture
ARCH=$(uname -m)
case $ARCH in
    x86_64) ARCH="x64" ;;
    aarch64) ARCH="arm64" ;;
esac

RELEASE_NAME="arclain-$VERSION-linux-$ARCH"
RELEASE_DIR="$REPO_ROOT/release/$RELEASE_NAME"

# Clean and create release directory
rm -rf "$RELEASE_DIR"
mkdir -p "$RELEASE_DIR"

# Copy binary
BINARY_PATH="$TARGET_DIR/release/arclain_ui"
if [ -f "$BINARY_PATH" ]; then
    cp "$BINARY_PATH" "$RELEASE_DIR/arclain"
    chmod +x "$RELEASE_DIR/arclain"
else
    echo "Error: Binary not found at $BINARY_PATH"
    exit 1
fi

# Copy plugins
PLUGINS_SOURCE="$REPO_ROOT/plugins"
PLUGINS_DEST="$RELEASE_DIR/plugins"
if [ -d "$PLUGINS_SOURCE" ]; then
    mkdir -p "$PLUGINS_DEST"
    find "$PLUGINS_SOURCE" -name "*.wasm" -exec cp {} "$PLUGINS_DEST/" \;
fi

# Create tarball
TARBALL="$REPO_ROOT/release/$RELEASE_NAME.tar.gz"
rm -f "$TARBALL"
tar -czf "$TARBALL" -C "$REPO_ROOT/release" "$RELEASE_NAME"

echo ""
echo "=== Release Complete ==="
echo "Package: $TARBALL"
echo "Version: $VERSION"
