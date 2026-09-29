#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
proxy="${P2PREMOTE_BUILD_PROXY:-${P2PREMOTE_BUILDER_PROXY:-}}"
arch="amd64"
rebuild=0

usage() {
  cat <<'EOF'
Usage: scripts/make_compile_image.sh [--arch amd64|arm64] [--proxy URL] [--rebuild]

Build the single Debian 10 compile image used by both GUI and headless builds.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch)
      case "${2:-}" in
        x86_64|amd64) arch="amd64" ;;
        aarch64|arm64) arch="arm64" ;;
        *) echo "--arch requires amd64 or arm64" >&2; exit 2 ;;
      esac
      shift 2
      ;;
    --proxy) proxy="${2:?missing value for --proxy}"; shift 2 ;;
    --rebuild) rebuild=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }

image="p2premote-linux-compile:debian10-rust1.77-v1-${arch}"
if [[ "$rebuild" == 0 ]] && docker image inspect "$image" >/dev/null 2>&1; then
  echo "==> Reusing compile image: $image"
  exit 0
fi

build_args=()
host_args=()
if [[ "$proxy" =~ ^(https?://)(127\.0\.0\.1|localhost)(:[0-9]+.*)$ ]]; then
  proxy="${BASH_REMATCH[1]}host.docker.internal${BASH_REMATCH[3]}"
  host_args=(--add-host=host.docker.internal:host-gateway)
fi
if [[ -n "$proxy" ]]; then
  for name in http_proxy https_proxy HTTP_PROXY HTTPS_PROXY; do
    build_args+=(--build-arg "$name=$proxy")
  done
fi
[[ -z "${P2PREMOTE_GO_DOWNLOAD_BASE:-}" ]] || build_args+=(--build-arg "GO_DOWNLOAD_BASE=$P2PREMOTE_GO_DOWNLOAD_BASE")
[[ -z "${P2PREMOTE_NODE_DOWNLOAD_BASE:-}" ]] || build_args+=(--build-arg "NODE_DOWNLOAD_BASE=$P2PREMOTE_NODE_DOWNLOAD_BASE")
# 海外 CI（GitHub Actions）访问 rsproxy.cn 经常超时：设置 RUSTUP_DIST_SERVER
# 即切换 rustup 下载源（如 https://static.rust-lang.org），与 Dockerfile ARG 同名
[[ -z "${RUSTUP_DIST_SERVER:-}" ]] || build_args+=(--build-arg "RUSTUP_DIST_SERVER=$RUSTUP_DIST_SERVER")
[[ -z "${RUSTUP_UPDATE_ROOT:-}" ]] || build_args+=(--build-arg "RUSTUP_UPDATE_ROOT=$RUSTUP_UPDATE_ROOT")

echo "==> Building Debian 10 compile image: $image"
docker build --network host --platform "linux/$arch" \
  "${host_args[@]}" "${build_args[@]}" \
  -t "$image" "$repo_dir/packaging/linux/builder"

docker run --rm --platform "linux/$arch" "$image" bash -c \
  'grep -q "VERSION_ID=\"10\"" /etc/os-release && rustc --version && cargo --version && go version && node --version && pkg-config --modversion webkit2gtk-4.0'
echo "==> Compile image is ready: $image"
