#!/bin/bash
set -e

PLUGINS_DIR="plugins"
TARGET="wasm32-unknown-unknown"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "🔧 Building WASM plugins..."
echo "Project root: $PROJECT_ROOT"

cd "$PROJECT_ROOT"

# Check if wasm32-unknown-unknown target is installed
if ! rustup target list --installed | grep -q "$TARGET"; then
    echo "⚠️  Target $TARGET not installed. Installing..."
    rustup target add $TARGET
fi

# Build each plugin
for plugin_dir in "$PLUGINS_DIR"/*/ ; do
    if [ -f "$plugin_dir/Cargo.toml" ]; then
        plugin_name=$(basename "$plugin_dir")
        echo ""
        echo "📦 Building $plugin_name..."
        
        cd "$plugin_dir"
        
        # Build for WASM
        cargo build --target $TARGET --release
        
        # Find the .wasm file using cargo metadata
        wasm_name=$(cargo metadata --no-deps --format-version 1 2>/dev/null | \
                    jq -r '.packages[0].targets[] | select(.kind[] == "cdylib") | .name' 2>/dev/null || \
                    echo "${plugin_name//-/_}")
        
        # WASM files are in workspace target directory
        wasm_src="../../target/$TARGET/release/${wasm_name}.wasm"
        wasm_dest="${plugin_name}.wasm"
        
        # Copy WASM file to plugin directory
        if [ -f "$wasm_src" ]; then
            cp "$wasm_src" "$wasm_dest"
            file_size=$(du -h "$wasm_dest" | cut -f1)
            echo "✅ Built: $wasm_dest ($file_size)"
        else
            echo "❌ Failed to find: $wasm_src"
            echo "   Tried: $wasm_src"
            echo "   Expected WASM output not found. Check build output above."
        fi
        
        cd "$PROJECT_ROOT"
    fi
done

echo ""
echo "🎉 All plugins built successfully!"
echo ""
echo "Plugin files created:"
for plugin_dir in "$PLUGINS_DIR"/*/ ; do
    plugin_name=$(basename "$plugin_dir")
    wasm_file="$plugin_dir/${plugin_name}.wasm"
    if [ -f "$wasm_file" ]; then
        file_size=$(du -h "$wasm_file" | cut -f1)
        echo "  - $wasm_file ($file_size)"
    fi
done