#!/usr/bin/env bash

set -euo pipefail
umask 022

usage() {
  echo "Usage: $0 -v <version> [--gnu|--target <rust-target>|--static-musl]" >&2
}

VERSION=""
TARGET_TRIPLE="${P2PREMOTE_LINUX_TARGET:-x86_64-unknown-linux-musl}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    -v)
      VERSION="${2:-}"
      shift 2
      ;;
    --target)
      TARGET_TRIPLE="${2:-}"
      shift 2
      ;;
    --static-musl)
      TARGET_TRIPLE="x86_64-unknown-linux-musl"
      shift
      ;;
    --gnu)
      TARGET_TRIPLE=""
      shift
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
TARGET_LABEL="x86_64-linux-gnu"
if [[ -n "$TARGET_TRIPLE" ]]; then
  TARGET_LABEL="${TARGET_TRIPLE//-/_}"
fi
case "${TARGET_TRIPLE:-$(uname -m)}" in
  *aarch64*|arm64)
    LINUX_ARCH="arm64"
    ;;
  *x86_64*|amd64)
    LINUX_ARCH="amd64"
    ;;
  *)
    echo "Unsupported Linux target architecture: ${TARGET_TRIPLE:-$(uname -m)}" >&2
    exit 1
    ;;
esac
if [[ -z "$TARGET_TRIPLE" && "$LINUX_ARCH" == "arm64" ]]; then
  TARGET_LABEL="aarch64-linux-gnu"
fi

rm -rf "$OUT_DIR" "$DIST_DIR"
mkdir -p "$OUT_DIR" "$DIST_DIR"

echo "==> Building static p2premote-punch"
"$APP_DIR/scripts/build-p2premote-punch.sh" -o "$PUNCH_LIB" -a "$LINUX_ARCH"

echo "==> Building userspace WireGuard fallback"
bash "$APP_DIR/scripts/build-wireguard-go.sh" -o "$WIREGUARD_GO" -a "$LINUX_ARCH"

echo "==> Building static WireGuard CLI"
bash "$APP_DIR/scripts/build-wireguard-tools.sh" -o "$WG_CLI" -a "$LINUX_ARCH"

echo "==> Building browser management UI"
(
  cd "$APP_DIR"
  npm ci
  npm run build
)

echo "==> Building Rust headless binaries"
(
  cd "$REPO_ROOT"
  if [[ -n "$TARGET_TRIPLE" ]]; then
    if ! rustup target list --installed | grep -qx "$TARGET_TRIPLE"; then
      echo "Rust target is not installed: $TARGET_TRIPLE" >&2
      echo "Run: rustup target add $TARGET_TRIPLE" >&2
      exit 1
    fi
    P2PREMOTE_CLIENT_VERSION="$BUILD_VERSION" cargo build --release --target "$TARGET_TRIPLE" -p p2premote-service -p p2premote-cli
  else
    P2PREMOTE_CLIENT_VERSION="$BUILD_VERSION" cargo build --release -p p2premote-service -p p2premote-cli
  fi
)

rust_release_dir() {
  if [[ -n "$TARGET_TRIPLE" ]]; then
    echo "$REPO_ROOT/target/$TARGET_TRIPLE/release"
  else
    echo "$REPO_ROOT/target/release"
  fi
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
