#!/usr/bin/env bash

# Build native headless packages and the Docker image as two independent
# compile targets. Each build bakes its own P2PREMOTE_RELEASE_TARGET into the
# service and CLI binaries.

set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: build-linux.sh -v <version> [options]

Options:
  -v <version>             SemVer version (required)
  --arch <x86_64|aarch64> Target architecture (default: host architecture)
  --tag <docker-tag>       Docker image tag
  --proxy <url>            Builder and Docker proxy
  --skip-web-build         Reuse the existing frontend dist directory
  --headless-only          Do not build the Docker image
  --docker-only            Do not build tar.gz/deb/rpm
  -h, --help               Show this help
EOF
}

VERSION=""
TARGET_ARCH="${P2PREMOTE_LINUX_ARCH:-}"
DOCKER_TAG=""
BUILDER_PROXY="${P2PREMOTE_BUILDER_PROXY:-${HTTPS_PROXY:-${https_proxy:-${HTTP_PROXY:-${http_proxy:-}}}}}"
SKIP_WEB_BUILD="${P2PREMOTE_SKIP_WEB_BUILD:-0}"
BUILD_HEADLESS=1
BUILD_DOCKER=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    -v) VERSION="${2:-}"; shift 2 ;;
    --arch) TARGET_ARCH="${2:-}"; shift 2 ;;
    --tag) DOCKER_TAG="${2:-}"; shift 2 ;;
    --proxy) BUILDER_PROXY="${2:-}"; shift 2 ;;
    --skip-web-build) SKIP_WEB_BUILD=1; shift ;;
    --headless-only) BUILD_DOCKER=0; shift ;;
    --docker-only) BUILD_HEADLESS=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 1 ;;
  esac
done

[[ -n "$VERSION" ]] || { usage; exit 1; }
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must be in semver format like 1.2.3" >&2; exit 1; }
case "${TARGET_ARCH:-$(uname -m)}" in
  x86_64|amd64) TARGET_ARCH="x86_64" ;;
  aarch64|arm64) TARGET_ARCH="aarch64" ;;
  *) echo "unsupported architecture: ${TARGET_ARCH:-$(uname -m)}" >&2; exit 1 ;;
esac
[[ "$BUILD_HEADLESS" == "1" || "$BUILD_DOCKER" == "1" ]] || { echo "--headless-only and --docker-only cannot be combined" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
APP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
COMMON_ARGS=(-v "$VERSION" --arch "$TARGET_ARCH")
[[ -z "$BUILDER_PROXY" ]] || COMMON_ARGS+=(--proxy "$BUILDER_PROXY")

# Builder containers run as root. Create every sibling artifact directory on
# the host before the first container starts, otherwise the first build can
# leave artifacts/ root-owned and prevent the later Docker build from creating
# its own output directory on GitHub runners and non-root Linux hosts.
mkdir -p "$APP_DIR/artifacts/linux-headless" "$APP_DIR/artifacts/linux-docker"

if [[ "$BUILD_HEADLESS" == "1" ]]; then
  echo "==> Building native Linux headless packages"
  if [[ "$SKIP_WEB_BUILD" == "1" ]]; then
    P2PREMOTE_SKIP_WEB_BUILD=1 "$SCRIPT_DIR/_build-linux-headless.sh" "${COMMON_ARGS[@]}"
  else
    "$SCRIPT_DIR/_build-linux-headless.sh" "${COMMON_ARGS[@]}"
  fi
fi

if [[ "$BUILD_DOCKER" == "1" ]]; then
  echo "==> Building independently compiled Docker image"
  DOCKER_ARGS=("${COMMON_ARGS[@]}")
  [[ -z "$DOCKER_TAG" ]] || DOCKER_ARGS+=(--tag "$DOCKER_TAG")
  if [[ "$SKIP_WEB_BUILD" == "1" || "$BUILD_HEADLESS" == "1" ]]; then
    DOCKER_ARGS+=(--skip-web-build)
  fi
  "$SCRIPT_DIR/_build-linux-docker.sh" "${DOCKER_ARGS[@]}"
fi

echo "==> Linux builds complete"
