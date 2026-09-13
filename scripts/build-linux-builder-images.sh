#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
proxy="${P2PREMOTE_BUILD_PROXY:-}"
export_dir=""
rebuild=0

usage() {
  cat <<'EOF'
Usage: scripts/build-linux-builder-images.sh [options]

Build reusable Linux toolchain images. The images include Rust 1.77.2 and
the GUI/headless native build dependencies, so normal builds do not download
the toolchains again.

Options:
  --proxy URL        Proxy used only while building the images
  --export-dir DIR   Also export compressed, portable image archives
  --rebuild          Rebuild images even when they already exist locally
  -h, --help         Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --proxy)
      proxy="${2:?missing value for --proxy}"
      shift 2
      ;;
    --export-dir)
      export_dir="${2:?missing value for --export-dir}"
      shift 2
      ;;
    --rebuild)
      rebuild=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }

gui_image="p2premote-tauri1-ubuntu18-rust1.77:v1"
headless_image="p2premote-linux-builder:glibc2.28-rust1.77-packages-v5-amd64"
build_args=()
if [[ -n "$proxy" ]]; then
  for name in http_proxy https_proxy HTTP_PROXY HTTPS_PROXY; do
    build_args+=(--build-arg "$name=$proxy")
  done
fi

if [[ "$rebuild" == 1 ]] || ! docker image inspect "$gui_image" >/dev/null 2>&1; then
  echo "==> Building reusable Ubuntu 18 GUI image: $gui_image"
  docker build --platform linux/amd64 \
    "${build_args[@]}" \
    -t "$gui_image" \
    -f "$repo_dir/packaging/linux/gui-ubuntu18/Dockerfile" \
    "$repo_dir/packaging/linux/gui-ubuntu18"
else
  echo "==> Reusing existing Ubuntu 18 GUI image: $gui_image"
fi

if [[ "$rebuild" == 1 ]] || ! docker image inspect "$headless_image" >/dev/null 2>&1; then
  echo "==> Building reusable glibc 2.28 headless image: $headless_image"
  docker build --network host --platform linux/amd64 \
    "${build_args[@]}" \
    -t "$headless_image" \
    "$repo_dir/packaging/linux/builder"
else
  echo "==> Reusing existing glibc 2.28 headless image: $headless_image"
fi

echo "==> Verifying pinned toolchains"
for image in "$gui_image" "$headless_image"; do
  docker run --rm --platform linux/amd64 "$image" \
    bash -c 'rustc --version; cargo --version; go version'
done

if [[ -n "$export_dir" ]]; then
  mkdir -p "$export_dir"
  gui_archive="$export_dir/p2premote-tauri1-ubuntu18-rust1.77-v1-amd64.tar.gz"
  headless_archive="$export_dir/p2premote-linux-builder-glibc2.28-rust1.77-v5-amd64.tar.gz"
  echo "==> Exporting $gui_archive"
  docker save "$gui_image" | gzip -1 > "$gui_archive"
  echo "==> Exporting $headless_archive"
  docker save "$headless_image" | gzip -1 > "$headless_archive"
  sha256sum "$gui_archive" "$headless_archive"
  echo "Import on another machine with: gzip -dc <archive.tar.gz> | docker load"
fi

echo "==> Linux builder images are ready"
docker image inspect --format '{{.RepoTags}} {{.Id}} {{.Size}}' \
  "$gui_image" "$headless_image"
