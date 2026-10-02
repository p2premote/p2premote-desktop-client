#!/usr/bin/env bash

# Compile through the proven headless pipeline with linux-docker-* baked in,
# then feed that independently compiled tarball to the original image builder.

set -euo pipefail
umask 022

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
command -v docker >/dev/null 2>&1 || { echo "Docker is required" >&2; exit 1; }

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GIT_COMMIT="$(git -C "$REPO_ROOT" rev-parse --short=6 HEAD)"
BUILD_VERSION="${VERSION}-${GIT_COMMIT}"
TARGET_ARCH="${TARGET_ARCH:-$(uname -m)}"
case "$TARGET_ARCH" in
  x86_64|amd64)
    TARGET_ARCH="x86_64"
    LINUX_ARCH="amd64"
    TARGET_LABEL="x86_64-linux-gnu"
    ;;
  aarch64|arm64)
    TARGET_ARCH="aarch64"
    LINUX_ARCH="arm64"
    TARGET_LABEL="aarch64-linux-gnu"
    ;;
  *) echo "Unsupported Linux architecture: $TARGET_ARCH" >&2; exit 1 ;;
esac

DOCKER_PLATFORM="linux/${LINUX_ARCH}"
DOCKER_TAG="${DOCKER_TAG:-p2premote/client:${VERSION}}"
DIST_DIR="$REPO_ROOT/artifacts/linux-docker"
PACKAGE_TAR="$DIST_DIR/p2premote-docker_${BUILD_VERSION}_${TARGET_LABEL}.tar.gz"
IMAGE_TAR="$DIST_DIR/p2premote-headless-docker_${BUILD_VERSION}_${TARGET_LABEL}.tar"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${RUST_CACHE_ROOT:-/mnt/n/rust-cache}/docker-${LINUX_ARCH}}"

BUILD_ARGS=(-v "$VERSION" --arch "$TARGET_ARCH" --docker-package)
[[ -z "$BUILDER_PROXY" ]] || BUILD_ARGS+=(--proxy "$BUILDER_PROXY")
if [[ "$SKIP_WEB_BUILD" == "1" ]]; then
  P2PREMOTE_SKIP_WEB_BUILD=1 CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
    "$REPO_ROOT/scripts/_build-linux-headless.sh" "${BUILD_ARGS[@]}"
else
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
    "$REPO_ROOT/scripts/_build-linux-headless.sh" "${BUILD_ARGS[@]}"
fi

[[ -f "$PACKAGE_TAR" ]] || { echo "Docker package input not found: $PACKAGE_TAR" >&2; exit 1; }

DOCKER_HOST_ARGS=()
if [[ "$BUILDER_PROXY" =~ ^(https?://)(127\.0\.0\.1|localhost)(:[0-9]+.*)$ ]]; then
  BUILDER_PROXY="${BASH_REMATCH[1]}host.docker.internal${BASH_REMATCH[3]}"
  DOCKER_HOST_ARGS=(--add-host=host.docker.internal:host-gateway)
elif [[ "$BUILDER_PROXY" == *"host.docker.internal"* ]]; then
  DOCKER_HOST_ARGS=(--add-host=host.docker.internal:host-gateway)
fi

CONTEXT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/p2premote-docker-context.XXXXXX")"
IMAGE_TAR_TMP="${IMAGE_TAR}.tmp"
cleanup() {
  rm -rf -- "$CONTEXT_DIR"
  rm -f -- "$IMAGE_TAR_TMP"
}
trap cleanup EXIT
cp "$REPO_ROOT/packaging/linux/docker/Dockerfile" "$CONTEXT_DIR/Dockerfile"
cp "$PACKAGE_TAR" "$CONTEXT_DIR/p2premote-docker.tar.gz"

IMAGE_BUILD_ARGS=(--build-arg P2P_DOCKER_TGZ=p2premote-docker.tar.gz)
if [[ -n "$BUILDER_PROXY" ]]; then
  IMAGE_BUILD_ARGS+=(
    --build-arg "http_proxy=$BUILDER_PROXY"
    --build-arg "https_proxy=$BUILDER_PROXY"
    --build-arg "HTTP_PROXY=$BUILDER_PROXY"
    --build-arg "HTTPS_PROXY=$BUILDER_PROXY"
  )
fi

docker build --platform "$DOCKER_PLATFORM" "${DOCKER_HOST_ARGS[@]}" \
  "${IMAGE_BUILD_ARGS[@]}" -t "$DOCKER_TAG" -f "$CONTEXT_DIR/Dockerfile" "$CONTEXT_DIR"
docker save -o "$IMAGE_TAR_TMP" "$DOCKER_TAG"
mv -f "$IMAGE_TAR_TMP" "$IMAGE_TAR"
docker image inspect "$DOCKER_TAG" >/dev/null
echo "Docker artifact: $IMAGE_TAR"
