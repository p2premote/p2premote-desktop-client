#!/usr/bin/env bash
set -euo pipefail

config="$1"
repo_dir="$(pwd)"
cargo_target_dir="${CARGO_TARGET_DIR:-$repo_dir/target/linux-gui-x64}"
dist_dir="$repo_dir/artifacts/linux-gui-x64"
export CARGO_TARGET_DIR="$cargo_target_dir"
wg_ffi_source_dir="../p2premote-wg-ffi"
[[ -d "$wg_ffi_source_dir" ]] || {
  echo "p2premote-wg-ffi source directory not found: $wg_ffi_source_dir" >&2
  exit 1
}

cargo build --release --package p2premote-service --package p2premote-cli
install -m 0755 "$cargo_target_dir/release/p2premote-service" src-tauri/resources/p2premote-service
install -m 0755 "$cargo_target_dir/release/p2premote-cli" src-tauri/resources/p2premote-cli
bash scripts/build-wireguard-go.sh \
  -o src-tauri/resources/wireguard-go \
  -a amd64 \
  -s "$wg_ffi_source_dir"
bash scripts/build-wireguard-tools.sh \
  -o src-tauri/resources/wg \
  -a amd64

for resource in \
  src-tauri/resources/p2premote-service \
  src-tauri/resources/p2premote-cli \
  src-tauri/resources/wireguard-go \
  src-tauri/resources/wg; do
  file "$resource" | grep -q 'ELF 64-bit' || {
    echo "failed to prepare Linux resource: $resource" >&2
    exit 1
  }
done
cargo tauri build --config "$config"
bash scripts/repack-linux-gui-deb-ubuntu18.sh "$P2PREMOTE_CLIENT_VERSION"
mkdir -p "$dist_dir"
install -m 0644 \
  "$cargo_target_dir/release/bundle/deb/p2premote_${P2PREMOTE_CLIENT_VERSION}_amd64.deb" \
  "$dist_dir/"
install -m 0755 \
  "$cargo_target_dir/release/bundle/appimage/p2premote_${P2PREMOTE_CLIENT_VERSION}_amd64.AppImage" \
  "$dist_dir/"
rm -f \
  "$cargo_target_dir/release/bundle/deb/p2premote_${P2PREMOTE_CLIENT_VERSION}_amd64.deb" \
  "$cargo_target_dir/release/bundle/appimage/p2premote_${P2PREMOTE_CLIENT_VERSION}_amd64.AppImage"
echo "Artifacts: $dist_dir"
