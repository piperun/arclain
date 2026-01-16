#!/bin/bash
set -e

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
CONFIG_FILE="$SCRIPT_DIR/logging_config.json"

# Default RUST_LOG
RUST_LOG="arclain=debug,info"

if [ -f "$CONFIG_FILE" ]; then
    echo "Reading logging config from $CONFIG_FILE"
    
    # Try to use python to parse JSON (cross-platform way without jq dependency)
    PARSED_LOG=$(python3 -c "
import json, sys
try:
    with open('$CONFIG_FILE') as f:
        data = json.load(f)
        parts = [data.get('default_level', 'info')]
        filters = data.get('filters', {})
        for k, v in filters.items():
            parts.append(f'{k}={v}')
        print(','.join(parts))
except Exception as e:
    sys.exit(1)
" 2>/dev/null)

    if [ $? -eq 0 ]; then
        RUST_LOG="$PARSED_LOG"
    else
        echo "Warning: Python not found or failed to parse config. Using default."
    fi
else
    echo "logging_config.json not found, using default."
fi

echo "Setting RUST_LOG=$RUST_LOG"
export RUST_LOG
export CARGO_TERM_COLOR=always

cd "$PROJECT_ROOT"
# cargo ui is an alias, we can run it directly or cargo run -p arclain_ui
cargo ui "$@"
