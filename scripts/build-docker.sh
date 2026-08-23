#!/usr/bin/env bash

# Compatibility entry point. Use build-linux.sh for the complete four-format
# Linux release; it compiles only once and exports all artifacts together.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
exec "$SCRIPT_DIR/build-linux.sh" "$@"
