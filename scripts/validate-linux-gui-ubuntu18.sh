#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || ! "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 <version>" >&2
  exit 2
fi
version="$1"
repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
source_root="$(cd "$repo_dir/.." && pwd)"
project_name="$(basename "$repo_dir")"
dist_dir="artifacts/linux-gui-x64"
deb="$dist_dir/p2premote_${version}_amd64.deb"
appimage="$dist_dir/p2premote_${version}_amd64.AppImage"
builder_image="${P2PREMOTE_GUI_BUILDER_IMAGE:-p2premote-tauri1-ubuntu18-rust1.77:v1}"

[[ -f "$repo_dir/$deb" ]] || { echo "missing $repo_dir/$deb" >&2; exit 1; }
[[ -f "$repo_dir/$appimage" ]] || { echo "missing $repo_dir/$appimage" >&2; exit 1; }

docker run --rm --platform linux/amd64 \
  -v "$source_root:/workspace" \
  -w "/workspace/$project_name" \
  "$builder_image" \
  bash scripts/validate-linux-gui-ubuntu18-container.sh "$deb" "$appimage"
