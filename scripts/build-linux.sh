#!/usr/bin/env bash

# Build all Linux delivery formats from one headless compilation:
#   - GNU/glibc tar.gz package
#   - deb package
#   - rpm package
#   - Docker image archive
#
# _build-linux-headless.sh remains the low-level producer for the first three
# packages. This script is the single user-facing entry point and reuses the
# exact tar.gz produced by that build as the Docker image input.

set -euo pipefail
umask 022

usage() {
  cat >&2 <<'EOF'
Usage: build-linux.sh -v <version> [options]

Options:
  -v <version>          SemVer version, for example 1.6.4 (required)
  --arch <x86_64|aarch64> Target architecture (default: host architecture)
  --tag <docker-tag>    Docker image tag (default: p2premote/client:<version>)
  --proxy <url>         HTTP(S) proxy used by the builder and Docker build
  --skip-web-build      Reuse the existing frontend dist directory
  --no-tgz-build        Compatibility mode: reuse an existing headless tgz
  --tgz-path <path>     Headless tgz to reuse with --no-tgz-build
  -h, --help            Show this help

The command emits four files under artifacts:
  linux-headless/*.tar.gz, linux-headless/*.deb, linux-headless/*.rpm,
  linux-headless/*.tar
EOF
}

VERSION=""
TARGET_ARCH="${P2PREMOTE_LINUX_ARCH:-}"
DOCKER_TAG=""
BUILDER_PROXY="${P2PREMOTE_BUILDER_PROXY:-${HTTPS_PROXY:-${https_proxy:-${HTTP_PROXY:-${http_proxy:-}}}}}"
SKIP_WEB_BUILD="${P2PREMOTE_SKIP_WEB_BUILD:-0}"
SKIP_HEADLESS_BUILD=0
EXPLICIT_TGZ=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    -v)
      [[ $# -ge 2 && -n "$2" ]] || { echo "-v requires a version value" >&2; exit 1; }
      VERSION="$2"
      shift 2
      ;;
    --arch)
      [[ $# -ge 2 && -n "$2" ]] || { echo "--arch requires x86_64 or aarch64" >&2; exit 1; }
      case "$2" in
        x86_64|amd64) TARGET_ARCH="x86_64" ;;
        aarch64|arm64) TARGET_ARCH="aarch64" ;;
        *)
          echo "unsupported --arch value: $2 (expected x86_64 or aarch64)" >&2
          exit 1
          ;;
      esac
      shift 2
      ;;
    --tag)
      [[ $# -ge 2 && -n "$2" ]] || { echo "--tag requires a value" >&2; exit 1; }
      DOCKER_TAG="$2"
      shift 2
      ;;
    --proxy)
      [[ $# -ge 2 && -n "$2" ]] || { echo "--proxy requires a proxy URL" >&2; exit 1; }
      BUILDER_PROXY="$2"
      shift 2
      ;;
    --skip-web-build)
      SKIP_WEB_BUILD=1
      shift
      ;;
    --no-tgz-build|--no-headless-build)
      SKIP_HEADLESS_BUILD=1
      shift
      ;;
    --tgz-path)
      [[ $# -ge 2 && -n "$2" ]] || { echo "--tgz-path requires a file path" >&2; exit 1; }
      EXPLICIT_TGZ="$2"
      SKIP_HEADLESS_BUILD=1
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage
      exit 1
      ;;
  esac
done

if [[ -z "$VERSION" ]]; then
  usage
  exit 1
fi
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "version must be in semver format like 1.2.3" >&2
  exit 1
fi
if [[ -n "$BUILDER_PROXY" && ! "$BUILDER_PROXY" =~ ^https?:// ]]; then
  echo "proxy URL must start with http:// or https://" >&2
  exit 1
fi
if [[ "$SKIP_WEB_BUILD" != "0" && "$SKIP_WEB_BUILD" != "1" ]]; then
  echo "--skip-web-build/P2PREMOTE_SKIP_WEB_BUILD must be 0 or 1" >&2
  exit 1
fi

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "This script must run in a Linux or WSL shell." >&2
  exit 1
fi
if ! command -v docker >/dev/null 2>&1; then
  echo "Docker is required to build and export the Docker image." >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
APP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST_DIR="$APP_DIR/artifacts"
HEADLESS_DIST_DIR="$DIST_DIR/linux-headless"
DOCKER_DIST_DIR="$HEADLESS_DIST_DIR"
DOCKERFILE="$APP_DIR/packaging/linux/docker/Dockerfile"
HEADLESS_SCRIPT="$SCRIPT_DIR/_build-linux-headless.sh"

[[ -f "$HEADLESS_SCRIPT" ]] || { echo "Headless build script not found: $HEADLESS_SCRIPT" >&2; exit 1; }
[[ -f "$DOCKERFILE" ]] || { echo "Dockerfile not found: $DOCKERFILE" >&2; exit 1; }

GIT_COMMIT="$(git -C "$APP_DIR" rev-parse --short=6 HEAD)"
if [[ ! "$GIT_COMMIT" =~ ^[0-9a-f]{6}$ ]]; then
  echo "Failed to resolve the current git commit id" >&2
  exit 1
fi
BUILD_VERSION="${VERSION}-${GIT_COMMIT}"

if [[ -z "$TARGET_ARCH" ]]; then
  TARGET_ARCH="$(uname -m)"
fi
case "$TARGET_ARCH" in
  x86_64|amd64)
    LINUX_ARCH="amd64"
    TARGET_LABEL="x86_64-linux-gnu"
    UPDATE_TARGET="linux-docker-x64"
    DEB_ARCH="amd64"
    RPM_ARCH="x86_64"
    ;;
  aarch64|arm64)
    LINUX_ARCH="arm64"
    TARGET_LABEL="aarch64-linux-gnu"
    UPDATE_TARGET="linux-docker-aarch64"
    DEB_ARCH="arm64"
    RPM_ARCH="aarch64"
    ;;
  *)
    echo "Unsupported Linux host architecture: $(uname -m)" >&2
    exit 1
    ;;
esac
DOCKER_PLATFORM="linux/${LINUX_ARCH}"

if [[ -z "$DOCKER_TAG" ]]; then
  DOCKER_TAG="p2premote/client:${VERSION}"
fi

# Make a WSL localhost proxy reachable from Docker containers/builds.
DOCKER_HOST_ARGS=()
if [[ "$BUILDER_PROXY" =~ ^(https?://)(127\.0\.0\.1|localhost)(:[0-9]+.*)$ ]]; then
  BUILDER_PROXY="${BASH_REMATCH[1]}host.docker.internal${BASH_REMATCH[3]}"
  DOCKER_HOST_ARGS=(--add-host=host.docker.internal:host-gateway)
  echo "==> Mapping the localhost proxy to host.docker.internal for Docker"
elif [[ "$BUILDER_PROXY" == *"host.docker.internal"* ]]; then
  DOCKER_HOST_ARGS=(--add-host=host.docker.internal:host-gateway)
fi

if [[ "$SKIP_HEADLESS_BUILD" -eq 0 ]]; then
  echo "==> Compiling once and creating tar.gz, deb and rpm"
  # Pre-create the output directory as the invoking user. Inside the builder
  # container mkdir -p runs as root; on a fresh CI workspace that would leave
  # the directory root-owned and the later host-side `docker save` could not
  # write its temp file (permission denied).
  mkdir -p "$HEADLESS_DIST_DIR"
  HEADLESS_ARGS=(-v "$VERSION" --arch "$TARGET_ARCH")
  if [[ -n "$BUILDER_PROXY" ]]; then
    HEADLESS_ARGS+=(--proxy "$BUILDER_PROXY")
  fi
  if [[ "$SKIP_WEB_BUILD" -eq 1 ]]; then
    P2PREMOTE_SKIP_WEB_BUILD=1 "$HEADLESS_SCRIPT" "${HEADLESS_ARGS[@]}"
  else
    "$HEADLESS_SCRIPT" "${HEADLESS_ARGS[@]}"
  fi
else
  echo "==> Reusing an existing headless tgz"
fi

# Fresh builds must use this commit's output, even when older releases remain.
# Reuse mode without an explicit path still requires an unambiguous archive.
if [[ -n "$EXPLICIT_TGZ" ]]; then
  TGZ_PATH="$EXPLICIT_TGZ"
elif [[ "$SKIP_HEADLESS_BUILD" -eq 0 ]]; then
  TGZ_PATH="$HEADLESS_DIST_DIR/p2premote-headless_${BUILD_VERSION}_${TARGET_LABEL}.tar.gz"
else
  shopt -s nullglob
  TGZ_CANDIDATES=("$HEADLESS_DIST_DIR"/p2premote-headless_"$VERSION"-*_"$TARGET_LABEL".tar.gz)
  shopt -u nullglob
  if (( ${#TGZ_CANDIDATES[@]} != 1 )); then
    echo "Expected exactly one ${TARGET_LABEL} headless tgz for version $VERSION under $HEADLESS_DIST_DIR; found ${#TGZ_CANDIDATES[@]}." >&2
    printf '  %s\n' "${TGZ_CANDIDATES[@]}" >&2
    exit 1
  fi
  TGZ_PATH="${TGZ_CANDIDATES[0]}"
fi

if [[ ! -f "$TGZ_PATH" ]]; then
  echo "headless tgz not found: $TGZ_PATH" >&2
  exit 1
fi
TGZ_NAME="$(basename "$TGZ_PATH")"
if [[ "$TGZ_NAME" != p2premote-headless_"$VERSION"-*_"$TARGET_LABEL".tar.gz ]]; then
  echo "headless tgz does not match version $VERSION and architecture $TARGET_LABEL: $TGZ_PATH" >&2
  exit 1
fi

mkdir -p "$DOCKER_DIST_DIR"
DOCKER_CONTEXT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/p2premote-docker-context.XXXXXX")"
IMAGE_TAR="$DOCKER_DIST_DIR/p2premote-headless-docker_${BUILD_VERSION}_${TARGET_LABEL}.tar"
IMAGE_TAR_TMP="${IMAGE_TAR}.tmp"
cleanup() {
  rm -rf -- "$DOCKER_CONTEXT_DIR"
  rm -f -- "$IMAGE_TAR_TMP"
}
trap cleanup EXIT

# Use a private context so the source tree is never modified by Docker
# packaging, and so concurrent builds cannot race on a shared temporary tgz.
cp "$DOCKERFILE" "$DOCKER_CONTEXT_DIR/Dockerfile"
cp "$TGZ_PATH" "$DOCKER_CONTEXT_DIR/p2premote-headless.tar.gz"

DOCKER_BUILD_ARGS=(
  "--build-arg" "P2P_HEADLESS_TGZ=p2premote-headless.tar.gz"
  "--build-arg" "P2PREMOTE_UPDATE_TARGET=$UPDATE_TARGET"
)
if [[ -n "$BUILDER_PROXY" ]]; then
  DOCKER_BUILD_ARGS+=(
    "--build-arg" "http_proxy=$BUILDER_PROXY"
    "--build-arg" "https_proxy=$BUILDER_PROXY"
    "--build-arg" "HTTP_PROXY=$BUILDER_PROXY"
    "--build-arg" "HTTPS_PROXY=$BUILDER_PROXY"
  )
fi
# 海外 CI 设置 RUSTUP_DIST_SERVER=https://static.rust-lang.org 切换 rustup
# 下载源（镜像默认 rsproxy.cn 在美国 runner 上经常超时）
if [[ -n "${RUSTUP_DIST_SERVER:-}" ]]; then
  DOCKER_BUILD_ARGS+=(
    "--build-arg" "RUSTUP_DIST_SERVER=$RUSTUP_DIST_SERVER"
    "--build-arg" "RUSTUP_UPDATE_ROOT=${RUSTUP_UPDATE_ROOT:-$RUSTUP_DIST_SERVER/rustup}"
  )
fi

echo "==> Building Docker image: $DOCKER_TAG"
docker build \
  --platform "$DOCKER_PLATFORM" \
  "${DOCKER_HOST_ARGS[@]}" \
  -t "$DOCKER_TAG" \
  -f "$DOCKER_CONTEXT_DIR/Dockerfile" \
  "${DOCKER_BUILD_ARGS[@]}" \
  "$DOCKER_CONTEXT_DIR"

echo "==> Exporting Docker image archive"
docker save -o "$IMAGE_TAR_TMP" "$DOCKER_TAG"
mv -f "$IMAGE_TAR_TMP" "$IMAGE_TAR"
docker image inspect "$DOCKER_TAG" >/dev/null

echo "==> Done: four Linux artifacts"
echo "  $HEADLESS_DIST_DIR/p2premote-headless_${BUILD_VERSION}_${TARGET_LABEL}.tar.gz"
echo "  $HEADLESS_DIST_DIR/p2premote-headless_${BUILD_VERSION}_${DEB_ARCH}.deb"
echo "  $HEADLESS_DIST_DIR/p2premote-headless-${VERSION}-1.${GIT_COMMIT}.${RPM_ARCH}.rpm"
echo "  $IMAGE_TAR"
