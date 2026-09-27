#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || ! "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 <version>" >&2
  exit 2
fi
version="$1"
# Rust 编译缓存根目录（N 盘不可用时改为对应盘符挂载路径，如 /mnt/d/rust-cache）
RUST_CACHE_ROOT="${RUST_CACHE_ROOT:-/mnt/n/rust-cache}"
cargo_target_dir="${CARGO_TARGET_DIR:-$RUST_CACHE_ROOT/wsl/linux-gui-x64}"
repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
source_root="$(cd "$repo_dir/.." && pwd)"
project_name="$(basename "$repo_dir")"

command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }
builder_image="p2premote-linux-compile:debian10-rust1.77-v1-amd64"
if ! docker image inspect "$builder_image" >/dev/null 2>&1; then
  echo "==> Compile image $builder_image not found; creating it first"
  image_args=(--arch amd64)
  [[ -z "${P2PREMOTE_BUILD_PROXY:-}" ]] || image_args+=(--proxy "$P2PREMOTE_BUILD_PROXY")
  "$repo_dir/scripts/make_compile_image.sh" "${image_args[@]}"
else
  echo "==> Reusing compile image $builder_image"
fi

docker_env=(
  -e P2PREMOTE_CLIENT_VERSION="$version"
  -e P2PREMOTE_PREBUILT_RESOURCES=1
  -e CARGO_NET_GIT_FETCH_WITH_CLI=true
  -e TAURI_TRAY=appindicator
  -e CARGO_TARGET_DIR="$cargo_target_dir"
  -e CARGO_HTTP_TIMEOUT=600
  -e CARGO_HTTP_MULTIPLEXING=false
  -e CARGO_NET_RETRY=10
  -e CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse
  -e CARGO_REGISTRIES_CRATES_IO_INDEX=sparse+https://rsproxy.cn/index/
  -e P2PREMOTE_LINUX_GUI_VERSION="$version"
)
if [[ -n "${P2PREMOTE_BUILD_PROXY:-}" ]]; then
  for name in HTTP_PROXY HTTPS_PROXY ALL_PROXY http_proxy https_proxy all_proxy; do
    docker_env+=( -e "$name=$P2PREMOTE_BUILD_PROXY" )
  done
fi

mkdir -p "$cargo_target_dir"

docker run --rm --platform linux/amd64 \
  -v "$source_root:/workspace" \
  -v "$cargo_target_dir:$cargo_target_dir" \
  -v p2p-cargo-registry:/opt/rust/cargo/registry \
  -v p2p-cargo-git:/opt/rust/cargo/git \
  -v p2premote-go-ubuntu18:/root/go/pkg/mod \
  -v p2premote-tauri-cache-ubuntu18:/root/.cache/tauri \
  -w "/workspace/$project_name" \
  "${docker_env[@]}" \
  "$builder_image" \
  bash scripts/build-linux-gui-container.sh

"$repo_dir/scripts/validate-linux-gui.sh" "$version"
