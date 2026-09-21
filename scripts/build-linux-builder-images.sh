#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"

usage() {
  cat <<'EOF'
Usage: scripts/build-linux-builder-images.sh [options]

Compatibility wrapper. Builds the single Debian 10 GUI/headless compile image.

Options:
  --proxy URL        Proxy used only while building the images
  --rebuild          Rebuild images even when they already exist locally
  -h, --help         Show this help
EOF
}

exec "$script_dir/make_compile_image.sh" "$@"
