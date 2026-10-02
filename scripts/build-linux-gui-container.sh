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

# artifact_version（如 1.13.2-0ccec0b）是本脚本唯一的版本源：tauri config 决定
# deb/AppImage 文件名与 deb control Version，Cargo.toml 决定 GUI 应用内
# env!("CARGO_PKG_VERSION")（设置页显示与检查更新上报），两者必须与产物文件名
# 同源，否则会出现"包文件名是新版本、软件内仍是旧版本"的事故
artifact_version="${P2PREMOTE_LINUX_GUI_ARTIFACT_VERSION:?P2PREMOTE_LINUX_GUI_ARTIFACT_VERSION is required}"
config="src-tauri/tauri.debian10.generated.conf.json"
frontend_dir="$(mktemp -d /tmp/p2premote-frontend.XXXXXX)"
version_backup_dir="$(mktemp -d /tmp/p2premote-version-backup.XXXXXX)"
cleanup() {
  rm -f "$config"
  rm -rf "$frontend_dir"
  if [[ -d "$version_backup_dir" ]]; then
    cp "$version_backup_dir/Cargo.toml" src-tauri/Cargo.toml 2>/dev/null || true
    cp "$version_backup_dir/Cargo.lock" Cargo.lock 2>/dev/null || true
    rm -rf "$version_backup_dir"
  fi
}
trap cleanup EXIT
node - "src-tauri/tauri.linux.conf.json" "$config" "$artifact_version" <<'NODE'
import fs from 'node:fs'
const [, , source, destination, version] = process.argv
const value = JSON.parse(fs.readFileSync(source, 'utf8'))
value.package = { ...(value.package ?? {}), version }
fs.writeFileSync(destination, `${JSON.stringify(value, null, 2)}\n`)
NODE

# src-tauri/Cargo.toml 临时盖章（与 build-windows.ps1 的 Set-ClientVersion 同款
# 机制），构建结束由 trap 还原，避免弄脏挂载进来的宿主工作树；Cargo.lock 会随
# cargo 命令连带更新，一并备份还原
version_field_count="$(grep -c '^version = "' src-tauri/Cargo.toml || true)"
if [[ "$version_field_count" -ne 1 ]]; then
  echo "expected exactly one top-level version field in src-tauri/Cargo.toml, found $version_field_count" >&2
  exit 1
fi
cp src-tauri/Cargo.toml "$version_backup_dir/Cargo.toml"
cp Cargo.lock "$version_backup_dir/Cargo.lock"
sed -i "s/^version = \".*\"/version = \"$artifact_version\"/" src-tauri/Cargo.toml
echo "==> Stamped src-tauri/Cargo.toml version to $artifact_version"

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

# 防回归：GUI 主程序与内嵌 service 必须包含本次构建版本。历史上 tag 触发的
# 构建曾因 Cargo.toml 未盖章出现"文件名 1.13.x、软件内 1.12.4-fb23c3"
for embedded_binary in \
  "$cargo_target_dir/release/p2premote" \
  src-tauri/resources/p2premote-service; do
  grep -aqF "$artifact_version" "$embedded_binary" || {
    echo "binary does not embed build version $artifact_version: $embedded_binary" >&2
    exit 1
  }
done

bash scripts/repack-linux-gui-deb.sh "$artifact_version"
mkdir -p "$dist_dir"
install -m 0644 \
  "$cargo_target_dir/release/bundle/deb/p2premote_${artifact_version}_amd64.deb" \
  "$dist_dir/p2premote_${artifact_version}_amd64.deb"
install -m 0755 \
  "$cargo_target_dir/release/bundle/appimage/p2premote_${artifact_version}_amd64.AppImage" \
  "$dist_dir/p2premote_${artifact_version}_amd64.AppImage"
rm -f \
  "$cargo_target_dir/release/bundle/deb/p2premote_${artifact_version}_amd64.deb" \
  "$cargo_target_dir/release/bundle/appimage/p2premote_${artifact_version}_amd64.AppImage"
echo "Artifacts: $dist_dir"
