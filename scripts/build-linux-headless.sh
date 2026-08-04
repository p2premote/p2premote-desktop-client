#!/usr/bin/env bash

set -euo pipefail
umask 022

usage() {
  echo "Usage: $0 -v <version>" >&2
}

VERSION=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -v)
      VERSION="${2:-}"
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

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "This script must run in a Linux or WSL shell." >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="$REPO_ROOT"
GIT_COMMIT="$(git -C "$REPO_ROOT" rev-parse --short=6 HEAD)"
if [[ ! "$GIT_COMMIT" =~ ^[0-9a-f]{6}$ ]]; then
  echo "Failed to resolve the current git commit id" >&2
  exit 1
fi
BUILD_VERSION="${VERSION}-${GIT_COMMIT}"
OUT_DIR="$APP_DIR/build/linux/headless"
DIST_DIR="$APP_DIR/build/linux/dist/headless"
PKG_ROOT="$OUT_DIR/package/p2premote-headless"
PUNCH_LIB="$APP_DIR/src-tauri/resources/libp2premote-punch.a"
WIREGUARD_GO="$OUT_DIR/wireguard-go"
WG_CLI="$OUT_DIR/wg"
case "$(uname -m)" in
  *aarch64*|arm64)
    LINUX_ARCH="arm64"
    TARGET_LABEL="aarch64-linux-gnu"
    ;;
  *x86_64*|amd64)
    LINUX_ARCH="amd64"
    TARGET_LABEL="x86_64-linux-gnu"
    ;;
  *)
    echo "Unsupported Linux host architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

rm -rf "$OUT_DIR" "$DIST_DIR"
mkdir -p "$OUT_DIR" "$DIST_DIR"

echo "==> Building static p2premote-punch"
"$APP_DIR/scripts/build-p2premote-punch.sh" -o "$PUNCH_LIB" -a "$LINUX_ARCH"

echo "==> Building userspace WireGuard fallback"
bash "$APP_DIR/scripts/build-wireguard-go.sh" -o "$WIREGUARD_GO" -a "$LINUX_ARCH"

echo "==> Building static WireGuard CLI"
bash "$APP_DIR/scripts/build-wireguard-tools.sh" -o "$WG_CLI" -a "$LINUX_ARCH"

echo "==> Building browser management UI"
if ! command -v node >/dev/null 2>&1; then
  echo "Node.js 22 or newer is required to build the browser management UI." >&2
  exit 1
fi
NODE_MAJOR="$(node -p 'process.versions.node.split(".")[0]')"
if [[ ! "$NODE_MAJOR" =~ ^[0-9]+$ ]] || (( NODE_MAJOR < 22 )); then
  echo "Node.js 22 or newer is required (found $(node --version))." >&2
  exit 1
fi
(
  cd "$APP_DIR"
  npm ci
  npm run build
)

echo "==> Building Rust headless binaries"
(
  cd "$REPO_ROOT"
  P2PREMOTE_CLIENT_VERSION="$BUILD_VERSION" cargo build --release -p p2premote-service -p p2premote-cli
)

rust_release_dir() {
  echo "$REPO_ROOT/target/release"
}

prepare_prefix_root() {
  local root="$1"
  local release_dir
  release_dir="$(rust_release_dir)"
  mkdir -p "$root/resources"
  mkdir -p "$root/resources/web"
  cp "$release_dir/p2premote-service" "$root/resources/p2premote-service"
  cp "$release_dir/p2premote-cli" "$root/resources/p2premote-cli"
  cp "$WIREGUARD_GO" "$root/resources/wireguard-go"
  cp "$WG_CLI" "$root/resources/wg"
  cp "$APP_DIR/src-tauri/resources/.p2premote_default.json" "$root/resources/.p2premote_default.json"
  cp -a "$APP_DIR/dist/." "$root/resources/web/"
  cp "$APP_DIR/packaging/linux/headless/scripts/install-service.sh" "$root/install-service.sh"
  cp "$APP_DIR/packaging/linux/headless/scripts/uninstall-service.sh" "$root/uninstall-service.sh"
  chmod 755 "$root/resources/p2premote-service" "$root/resources/p2premote-cli" "$root/resources/wireguard-go" "$root/resources/wg"
  chmod 755 "$root/install-service.sh" "$root/uninstall-service.sh"
  chmod -R a+rX "$root/resources/web"
}

echo "==> Preparing headless tarball root"
prepare_prefix_root "$PKG_ROOT"

echo "==> Building tar.gz package"
tar -C "$OUT_DIR/package" -czf "$DIST_DIR/p2premote-headless_${BUILD_VERSION}_${TARGET_LABEL}.tar.gz" p2premote-headless

echo "==> Done"
echo "Artifacts: $DIST_DIR"
