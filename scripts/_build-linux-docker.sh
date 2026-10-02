#!/usr/bin/env bash

set -euo pipefail
umask 022

RUST_CACHE_ROOT="${RUST_CACHE_ROOT:-/mnt/n/rust-cache}"

usage() {
  echo "Usage: $0 -v <version> [--arch <x86_64|aarch64>] [--tag <docker-tag>] [--proxy <http-proxy-url>] [--skip-web-build] [--no-sccache]" >&2
}

VERSION=""
TARGET_ARCH="${P2PREMOTE_LINUX_ARCH:-}"
DOCKER_TAG=""
BUILDER_PROXY="${P2PREMOTE_BUILDER_PROXY:-${HTTPS_PROXY:-${https_proxy:-${HTTP_PROXY:-${http_proxy:-}}}}}"
SKIP_WEB_BUILD="${P2PREMOTE_SKIP_WEB_BUILD:-0}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    -v) VERSION="${2:-}"; shift 2 ;;
    --arch)
      case "${2:-}" in
        x86_64|amd64) TARGET_ARCH="x86_64" ;;
        aarch64|arm64) TARGET_ARCH="aarch64" ;;
        *) echo "unsupported --arch value: ${2:-}" >&2; exit 1 ;;
      esac
      shift 2
      ;;
    --tag) DOCKER_TAG="${2:-}"; shift 2 ;;
    --proxy) BUILDER_PROXY="${2:-}"; shift 2 ;;
    --skip-web-build) SKIP_WEB_BUILD=1; shift ;;
    --no-sccache) shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 1 ;;
  esac
done

[[ -n "$VERSION" ]] || { usage; exit 1; }
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must be in semver format like 1.2.3" >&2; exit 1; }
[[ -z "$BUILDER_PROXY" || "$BUILDER_PROXY" =~ ^https?:// ]] || { echo "proxy URL must start with http:// or https://" >&2; exit 1; }
[[ "$(uname -s)" == "Linux" ]] || { echo "This script must run in a Linux or WSL shell." >&2; exit 1; }

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GIT_COMMIT="$(git -C "$REPO_ROOT" rev-parse --short=6 HEAD)"
[[ "$GIT_COMMIT" =~ ^[0-9a-f]{6}$ ]] || { echo "Failed to resolve the current git commit id" >&2; exit 1; }
BUILD_VERSION="${VERSION}-${GIT_COMMIT}"
TARGET_ARCH="${TARGET_ARCH:-$(uname -m)}"

case "$TARGET_ARCH" in
  x86_64|amd64)
    TARGET_ARCH="x86_64"
    LINUX_ARCH="amd64"
    RUST_TARGET="x86_64-unknown-linux-musl"
    TARGET_LABEL="x86_64-linux-gnu"
    RELEASE_TARGET="linux-docker-x64"
    ;;
  aarch64|arm64)
    TARGET_ARCH="aarch64"
    LINUX_ARCH="arm64"
    RUST_TARGET="aarch64-unknown-linux-musl"
    TARGET_LABEL="aarch64-linux-gnu"
    RELEASE_TARGET="linux-docker-aarch64"
    ;;
  *) echo "Unsupported Linux architecture: $TARGET_ARCH" >&2; exit 1 ;;
esac

DOCKER_PLATFORM="linux/${LINUX_ARCH}"
DOCKER_TAG="${DOCKER_TAG:-p2premote/client:${VERSION}}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$RUST_CACHE_ROOT/docker-${LINUX_ARCH}}"
export CARGO_TARGET_DIR
DIST_DIR="$REPO_ROOT/artifacts/linux-docker"
CONTEXT_DIR="$DIST_DIR/context-${LINUX_ARCH}"
IMAGE_TAR="$DIST_DIR/p2premote-headless-docker_${BUILD_VERSION}_${TARGET_LABEL}.tar"

DOCKER_HOST_ARGS=()
if [[ "$BUILDER_PROXY" =~ ^(https?://)(127\.0\.0\.1|localhost)(:[0-9]+.*)$ ]]; then
  BUILDER_PROXY="${BASH_REMATCH[1]}host.docker.internal${BASH_REMATCH[3]}"
  DOCKER_HOST_ARGS=(--add-host=host.docker.internal:host-gateway)
elif [[ "$BUILDER_PROXY" == *"host.docker.internal"* ]]; then
  DOCKER_HOST_ARGS=(--add-host=host.docker.internal:host-gateway)
fi

if [[ "${P2PREMOTE_IN_BUILDER_CONTAINER:-0}" != "1" ]]; then
  command -v docker >/dev/null 2>&1 || { echo "Docker is required" >&2; exit 1; }
  BUILDER_IMAGE="p2premote-linux-compile:debian10-rust1.77-v1-${LINUX_ARCH}"
  if ! docker image inspect "$BUILDER_IMAGE" >/dev/null 2>&1; then
    IMAGE_ARGS=(--arch "$LINUX_ARCH")
    [[ -z "$BUILDER_PROXY" ]] || IMAGE_ARGS+=(--proxy "$BUILDER_PROXY")
    "$REPO_ROOT/scripts/make_compile_image.sh" "${IMAGE_ARGS[@]}"
  fi

  WORKSPACE_DIR="$(cd "$REPO_ROOT/.." && pwd)"
  mkdir -p "$CARGO_TARGET_DIR" "$DIST_DIR"
  RUN_ENV=(-e P2PREMOTE_IN_BUILDER_CONTAINER=1 -e RUSTUP_TOOLCHAIN=1.77.2 -e "CARGO_TARGET_DIR=$CARGO_TARGET_DIR")
  [[ "$SKIP_WEB_BUILD" != "1" ]] || RUN_ENV+=(-e P2PREMOTE_SKIP_WEB_BUILD=1)
  if [[ -n "$BUILDER_PROXY" ]]; then
    RUN_ENV+=(-e "http_proxy=$BUILDER_PROXY" -e "https_proxy=$BUILDER_PROXY" -e "HTTP_PROXY=$BUILDER_PROXY" -e "HTTPS_PROXY=$BUILDER_PROXY")
  fi

  docker run --rm --network host --platform "$DOCKER_PLATFORM" \
    "${DOCKER_HOST_ARGS[@]}" \
    -v "$WORKSPACE_DIR:/workspace" \
    -v "$CARGO_TARGET_DIR:$CARGO_TARGET_DIR" \
    -v p2p-cargo-registry:/opt/rust/cargo/registry \
    -v p2p-cargo-git:/opt/rust/cargo/git \
    -v p2p-npm-cache:/root/.npm \
    -v p2p-go-mod:/root/go \
    -v p2p-go-build:/root/.cache/go-build \
    -w /workspace/p2premote-desktop-client \
    "${RUN_ENV[@]}" "$BUILDER_IMAGE" \
    bash -c "git config --global --add safe.directory '*' && ./scripts/_build-linux-docker.sh -v '${VERSION}' --arch '${TARGET_ARCH}' --no-sccache"

  [[ -f "$CONTEXT_DIR/package/resources/p2premote-service" ]] || { echo "Docker build context was not produced" >&2; exit 1; }

  BUILD_ARGS=()
  if [[ -n "$BUILDER_PROXY" ]]; then
    BUILD_ARGS+=(--build-arg "http_proxy=$BUILDER_PROXY" --build-arg "https_proxy=$BUILDER_PROXY" --build-arg "HTTP_PROXY=$BUILDER_PROXY" --build-arg "HTTPS_PROXY=$BUILDER_PROXY")
  fi
  docker build --platform "$DOCKER_PLATFORM" "${DOCKER_HOST_ARGS[@]}" "${BUILD_ARGS[@]}" -t "$DOCKER_TAG" -f "$CONTEXT_DIR/Dockerfile" "$CONTEXT_DIR"
  docker save -o "${IMAGE_TAR}.tmp" "$DOCKER_TAG"
  mv -f "${IMAGE_TAR}.tmp" "$IMAGE_TAR"
  echo "Docker artifact: $IMAGE_TAR"
  exit 0
fi

export RUSTC_WRAPPER=""
export CARGO_NET_GIT_FETCH_WITH_CLI=true
export CARGO_HTTP_TIMEOUT="${CARGO_HTTP_TIMEOUT:-600}"
export CARGO_NET_RETRY="${CARGO_NET_RETRY:-10}"

PUNCH_RS_SOURCE_DIR="$REPO_ROOT/../p2premote-punch-rs"
WG_FFI_SOURCE_DIR="$REPO_ROOT/../p2premote-wg-ffi"
[[ -d "$PUNCH_RS_SOURCE_DIR" ]] || { echo "p2premote-punch-rs source directory not found" >&2; exit 1; }
[[ -d "$WG_FFI_SOURCE_DIR" ]] || { echo "p2premote-wg-ffi source directory not found" >&2; exit 1; }

OUT_DIR="$CARGO_TARGET_DIR/package-assets/linux-docker-${LINUX_ARCH}"
rm -rf "$OUT_DIR" "$CONTEXT_DIR"
mkdir -p "$OUT_DIR" "$CONTEXT_DIR/package/resources/web" "$CONTEXT_DIR/package/data" "$CONTEXT_DIR/package/logs" "$CONTEXT_DIR/package/run"

bash "$REPO_ROOT/scripts/build-wireguard-go.sh" -o "$OUT_DIR/wireguard-go" -a "$LINUX_ARCH" -s "$WG_FFI_SOURCE_DIR"
bash "$REPO_ROOT/scripts/build-wireguard-tools.sh" -o "$OUT_DIR/wg" -a "$LINUX_ARCH"

if [[ "$SKIP_WEB_BUILD" == "1" ]]; then
  [[ -f "$REPO_ROOT/dist/index.html" ]] || { echo "--skip-web-build requires $REPO_ROOT/dist/index.html" >&2; exit 1; }
else
  frontend_stage="$(mktemp -d /tmp/p2premote-docker-frontend.XXXXXX)"
  cp "$REPO_ROOT"/{package.json,package-lock.json,index.html,tsconfig.json,tsconfig.node.json,vite.config.ts} "$frontend_stage/"
  cp -a "$REPO_ROOT/src" "$REPO_ROOT/public" "$frontend_stage/"
  (cd "$frontend_stage" && npm ci && npm run build)
  rm -rf "$REPO_ROOT/dist"
  cp -a "$frontend_stage/dist/." "$REPO_ROOT/dist/"
  rm -rf "$frontend_stage"
fi

(cd "$REPO_ROOT" && P2PREMOTE_CLIENT_VERSION="$BUILD_VERSION" P2PREMOTE_RELEASE_TARGET="$RELEASE_TARGET" cargo build --release --target "$RUST_TARGET" -p p2premote-service -p p2premote-cli)

RELEASE_DIR="$CARGO_TARGET_DIR/$RUST_TARGET/release"
cp "$RELEASE_DIR/p2premote-service" "$CONTEXT_DIR/package/resources/"
cp "$RELEASE_DIR/p2premote-cli" "$CONTEXT_DIR/package/resources/"
cp "$OUT_DIR/wireguard-go" "$CONTEXT_DIR/package/resources/"
cp "$OUT_DIR/wg" "$CONTEXT_DIR/package/resources/"
cp "$REPO_ROOT/src-tauri/resources/.p2premote_default.json" "$CONTEXT_DIR/package/resources/"
cp -a "$REPO_ROOT/dist/." "$CONTEXT_DIR/package/resources/web/"
cp "$REPO_ROOT/packaging/linux/docker/Dockerfile" "$CONTEXT_DIR/Dockerfile"
chmod 755 "$CONTEXT_DIR/package/resources/p2premote-service" "$CONTEXT_DIR/package/resources/p2premote-cli" "$CONTEXT_DIR/package/resources/wireguard-go" "$CONTEXT_DIR/package/resources/wg"
