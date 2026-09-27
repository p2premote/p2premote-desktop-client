#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(pwd)"
# Rust 编译缓存根目录（由宿主脚本通过 CARGO_TARGET_DIR 注入；此处为独立运行时的默认值）
RUST_CACHE_ROOT="${RUST_CACHE_ROOT:-/mnt/n/rust-cache}"
cargo_target_dir="${CARGO_TARGET_DIR:-$RUST_CACHE_ROOT/wsl/linux-gui-x64}"
dist_dir="$repo_dir/artifacts/linux-gui-x64"
export CARGO_TARGET_DIR="$cargo_target_dir"
wg_ffi_source_dir="../p2premote-wg-ffi"
[[ -d "$wg_ffi_source_dir" ]] || {
  echo "p2premote-wg-ffi source directory not found: $wg_ffi_source_dir" >&2
  exit 1
}

version="${P2PREMOTE_LINUX_GUI_VERSION:?P2PREMOTE_LINUX_GUI_VERSION is required}"
artifact_version="${P2PREMOTE_LINUX_GUI_ARTIFACT_VERSION:?P2PREMOTE_LINUX_GUI_ARTIFACT_VERSION is required}"
config="src-tauri/tauri.debian10.generated.conf.json"
frontend_dir="$(mktemp -d /tmp/p2premote-frontend.XXXXXX)"
node - "src-tauri/tauri.linux.conf.json" "$config" "$version" <<'NODE'
import fs from 'node:fs'
const [, , source, destination, version] = process.argv
const value = JSON.parse(fs.readFileSync(source, 'utf8'))
value.package = { ...(value.package ?? {}), version }
fs.writeFileSync(destination, `${JSON.stringify(value, null, 2)}\n`)
NODE
cleanup() {
  rm -f "$config"
  rm -rf "$frontend_dir"
}
trap cleanup EXIT

# Frontend compilation is also pinned to Debian 10; the host only supplies Docker.
cp "$repo_dir"/{package.json,package-lock.json,index.html,tsconfig.json,tsconfig.node.json,vite.config.ts} "$frontend_dir/"
cp -a "$repo_dir/src" "$repo_dir/public" "$frontend_dir/"
(cd "$frontend_dir" && npm ci && npm run build)
rm -rf "$repo_dir/dist"
cp -a "$frontend_dir/dist" "$repo_dir/dist"
node scripts/prepare-web-resources.mjs

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
"$frontend_dir/node_modules/.bin/tauri" build --config "$config"
bash scripts/repack-linux-gui-deb.sh "$version"
mkdir -p "$dist_dir"
install -m 0644 \
  "$cargo_target_dir/release/bundle/deb/p2premote_${version}_amd64.deb" \
  "$dist_dir/p2premote_${artifact_version}_amd64.deb"
install -m 0755 \
  "$cargo_target_dir/release/bundle/appimage/p2premote_${version}_amd64.AppImage" \
  "$dist_dir/p2premote_${artifact_version}_amd64.AppImage"
rm -f \
  "$cargo_target_dir/release/bundle/deb/p2premote_${version}_amd64.deb" \
  "$cargo_target_dir/release/bundle/appimage/p2premote_${version}_amd64.AppImage"
echo "Artifacts: $dist_dir"
