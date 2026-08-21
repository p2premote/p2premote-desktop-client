#!/usr/bin/env bash

set -euo pipefail
umask 022

usage() {
  echo "Usage: $0 -v <version> [--proxy <http-proxy-url>]" >&2
}

VERSION=""
BUILDER_PROXY="${P2PREMOTE_BUILDER_PROXY:-${HTTPS_PROXY:-${https_proxy:-${HTTP_PROXY:-${http_proxy:-}}}}}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    -v)
      if [[ $# -lt 2 || -z "$2" ]]; then
        echo "-v requires a version value" >&2
        exit 1
      fi
      VERSION="$2"
      shift 2
      ;;
    --proxy)
      if [[ $# -lt 2 || -z "$2" ]]; then
        echo "--proxy requires a proxy URL" >&2
        exit 1
      fi
      BUILDER_PROXY="$2"
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

DOCKER_HOST_ARGS=()
if [[ "$BUILDER_PROXY" =~ ^(https?://)(127\.0\.0\.1|localhost)(:[0-9]+.*)$ ]]; then
  BUILDER_PROXY="${BASH_REMATCH[1]}host.docker.internal${BASH_REMATCH[3]}"
  DOCKER_HOST_ARGS=(--add-host=host.docker.internal:host-gateway)
  echo "==> Mapping the localhost proxy to host.docker.internal for Docker"
elif [[ "$BUILDER_PROXY" == *"host.docker.internal"* ]]; then
  DOCKER_HOST_ARGS=(--add-host=host.docker.internal:host-gateway)
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
PUNCH_LIB="$APP_DIR/src-tauri/resources/libp2premote-punch.a"
PUNCH_SOURCE_DIR="$REPO_ROOT/../p2premote-punch"
WIREGUARD_GO="$OUT_DIR/wireguard-go"
WG_CLI="$OUT_DIR/wg"
case "$(uname -m)" in
  aarch64)
    LINUX_ARCH="arm64"
    TARGET_LABEL="aarch64-linux-gnu"
    DEB_ARCH="arm64"
    RPM_ARCH="aarch64"
    ;;
  x86_64)
    LINUX_ARCH="amd64"
    TARGET_LABEL="x86_64-linux-gnu"
    DEB_ARCH="amd64"
    RPM_ARCH="x86_64"
    ;;
  *)
    echo "Unsupported Linux host architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

# Headless 产物必须携带固定的 glibc 基线（Debian 10 / GLIBC_2.28），
# 因此编译一律在 builder 容器内完成；本机环境只提供源码和 Docker，
# 不允许用宿主机工具链直接编译。已在容器内时跳过本段。
if [[ "${P2PREMOTE_IN_BUILDER_CONTAINER:-0}" != "1" ]]; then
  if ! command -v docker >/dev/null 2>&1; then
    echo "Docker is required: the headless package must be built inside the" >&2
    echo "p2premote-linux-builder container to pin its glibc baseline." >&2
    exit 1
  fi
  if [[ "$(basename "$REPO_ROOT")" != "p2premote-desktop-client" ]]; then
    echo "Repository directory must be named p2premote-desktop-client (found $(basename "$REPO_ROOT")) so it can be mounted into the builder container." >&2
    exit 1
  fi

  BUILDER_IMAGE="p2premote-linux-builder:glibc2.28-packages-v2-${LINUX_ARCH}"
  if ! docker image inspect "$BUILDER_IMAGE" >/dev/null 2>&1; then
    echo "==> Builder image $BUILDER_IMAGE not found; building it first (one-time)"
    PROXY_ARGS=()
    if [[ -n "$BUILDER_PROXY" ]]; then
      echo "==> Using configured proxy for builder downloads"
      PROXY_ARGS=(--build-arg "http_proxy=${BUILDER_PROXY}"
        --build-arg "https_proxy=${BUILDER_PROXY}"
        --build-arg "HTTP_PROXY=${BUILDER_PROXY}"
        --build-arg "HTTPS_PROXY=${BUILDER_PROXY}")
    fi
    if [[ -n "${P2PREMOTE_GO_DOWNLOAD_BASE:-}" ]]; then
      PROXY_ARGS+=(--build-arg "GO_DOWNLOAD_BASE=${P2PREMOTE_GO_DOWNLOAD_BASE}")
    fi
    if [[ -n "${P2PREMOTE_NODE_DOWNLOAD_BASE:-}" ]]; then
      PROXY_ARGS+=(--build-arg "NODE_DOWNLOAD_BASE=${P2PREMOTE_NODE_DOWNLOAD_BASE}")
    fi
    docker build -t "$BUILDER_IMAGE" "${DOCKER_HOST_ARGS[@]}" ${PROXY_ARGS+"${PROXY_ARGS[@]}"} "$APP_DIR/packaging/linux/builder"
  fi

  RUN_ENV=(-e "P2PREMOTE_IN_BUILDER_CONTAINER=1")
  if [[ "${P2PREMOTE_SKIP_WEB_BUILD:-0}" == "1" ]]; then
    RUN_ENV+=(-e "P2PREMOTE_SKIP_WEB_BUILD=1")
  fi
  if [[ -n "$BUILDER_PROXY" ]]; then
    RUN_ENV+=(-e "http_proxy=${BUILDER_PROXY}" -e "https_proxy=${BUILDER_PROXY}"
      -e "HTTP_PROXY=${BUILDER_PROXY}" -e "HTTPS_PROXY=${BUILDER_PROXY}")
  fi

  WORKSPACE_DIR="$(cd "$REPO_ROOT/.." && pwd)"
  echo "==> Building inside $BUILDER_IMAGE (glibc 2.28 baseline, cached toolchains in volumes)"
  exec docker run --rm \
    "${DOCKER_HOST_ARGS[@]}" \
    -v "$WORKSPACE_DIR:/workspace" \
    -v "p2p-cargo-target-${LINUX_ARCH}:/workspace/p2premote-desktop-client/target" \
    -v p2p-cargo-registry:/opt/rust/cargo/registry \
    -v p2p-npm-cache:/root/.npm \
    -v p2p-go-mod:/root/go \
    -v p2p-go-build:/root/.cache/go-build \
    -w /workspace/p2premote-desktop-client \
    "${RUN_ENV[@]}" \
    "$BUILDER_IMAGE" \
    bash -c "git config --global --add safe.directory '*' && ./scripts/build-linux-headless.sh -v '${VERSION}'"
fi

if [[ ! -d "$PUNCH_SOURCE_DIR" ]]; then
  echo "p2premote-punch source directory not found: $PUNCH_SOURCE_DIR" >&2
  exit 1
fi

# WSL bind mounts such as /mnt/c and /mnt/d commonly expose every directory as
# mode 0777. dpkg-deb rejects a 0777 DEBIAN control directory, and rpmbuild also
# relies on native Unix ownership/modes. Stage packages on the container's Linux
# filesystem and copy only the finished archives back to the mounted workspace.
PACKAGE_STAGE_DIR=$(mktemp -d "/tmp/p2premote-package.XXXXXX")
PKG_ROOT="$PACKAGE_STAGE_DIR/tar/p2premote-headless"

rm -rf "$OUT_DIR" "$DIST_DIR"
mkdir -p "$OUT_DIR" "$DIST_DIR"

echo "==> Building static p2premote-punch"
"$APP_DIR/scripts/build-p2premote-punch.sh" -o "$PUNCH_LIB" -a "$LINUX_ARCH" -s "$PUNCH_SOURCE_DIR"

echo "==> Building userspace WireGuard implementation"
bash "$APP_DIR/scripts/build-wireguard-go.sh" -o "$WIREGUARD_GO" -a "$LINUX_ARCH" -s "$PUNCH_SOURCE_DIR"

echo "==> Building static WireGuard CLI"
bash "$APP_DIR/scripts/build-wireguard-tools.sh" -o "$WG_CLI" -a "$LINUX_ARCH"

echo "==> Building browser management UI"
if [[ "${P2PREMOTE_SKIP_WEB_BUILD:-0}" == "1" ]]; then
  if [[ ! -f "$APP_DIR/dist/index.html" ]]; then
    echo "P2PREMOTE_SKIP_WEB_BUILD=1 requires a prebuilt frontend under $APP_DIR/dist." >&2
    exit 1
  fi
  echo "Skipping browser management UI build; using existing $APP_DIR/dist"
else
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
fi

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
  cp "$APP_DIR/packaging/linux/common/configure-installation.sh" "$root/resources/configure-installation"
  cp -a "$APP_DIR/dist/." "$root/resources/web/"
  cp "$APP_DIR/packaging/linux/common/p2premote-service.service" "$root/p2premote-service.service"
  cp "$APP_DIR/packaging/linux/headless/scripts/install-service.sh" "$root/install-service.sh"
  cp "$APP_DIR/packaging/linux/headless/scripts/install-gui.sh" "$root/install-gui.sh"
  cp "$APP_DIR/packaging/linux/headless/scripts/install-p2premote.desktop" "$root/安装-p2pRemote.desktop"
  cp "$APP_DIR/packaging/linux/headless/scripts/uninstall-service.sh" "$root/uninstall-service.sh"
  chmod 755 "$root/resources/p2premote-service" "$root/resources/p2premote-cli" "$root/resources/wireguard-go" "$root/resources/wg" "$root/resources/configure-installation"
  chmod 755 "$root/install-service.sh" "$root/install-gui.sh" "$root/安装-p2pRemote.desktop" "$root/uninstall-service.sh"
  chmod 644 "$root/resources/.p2premote_default.json" "$root/p2premote-service.service"
  find "$root/resources/web" -type d -exec chmod 755 {} +
  find "$root/resources/web" -type f -exec chmod 644 {} +
}

echo "==> Preparing headless tarball root"
prepare_prefix_root "$PKG_ROOT"
# Source files arrive through a WSL bind mount and may retain the host user's
# numeric uid. Native packages must install application files as root-owned.
chown -R 0:0 "$PKG_ROOT"

echo "==> Building tar.gz package"
tar -C "$PACKAGE_STAGE_DIR/tar" -czf "$DIST_DIR/p2premote-headless_${BUILD_VERSION}_${TARGET_LABEL}.tar.gz" p2premote-headless

echo "==> Building deb package"
DEB_ROOT="$PACKAGE_STAGE_DIR/deb"
mkdir -p "$DEB_ROOT/DEBIAN" "$DEB_ROOT/opt/p2premote/resources" \
  "$DEB_ROOT/usr/lib/systemd/system" "$DEB_ROOT/usr/share/applications"
chmod 755 "$DEB_ROOT/DEBIAN"
cp -a "$PKG_ROOT/resources/." "$DEB_ROOT/opt/p2premote/resources/"
cp "$APP_DIR/packaging/linux/common/p2premote-service.service" "$DEB_ROOT/usr/lib/systemd/system/p2premote-service.service"
cp "$APP_DIR/packaging/linux/common/p2premote-web.desktop" "$DEB_ROOT/usr/share/applications/p2premote.desktop"
sed -e "s/@VERSION@/$BUILD_VERSION/g" -e "s/@ARCH@/$DEB_ARCH/g" \
  "$APP_DIR/packaging/linux/deb/control.in" > "$DEB_ROOT/DEBIAN/control"
for script in preinst postinst prerm postrm; do
  cp "$APP_DIR/packaging/linux/deb/$script" "$DEB_ROOT/DEBIAN/$script"
  chmod 755 "$DEB_ROOT/DEBIAN/$script"
done
DEB_FILE="$DIST_DIR/p2premote-headless_${BUILD_VERSION}_${DEB_ARCH}.deb"
dpkg-deb --root-owner-group --build "$DEB_ROOT" "$DEB_FILE"
dpkg-deb --info "$DEB_FILE" >/dev/null
if dpkg-deb --contents "$DEB_FILE" | awk '$2 != "root/root" { found=1 } END { exit found ? 0 : 1 }'; then
  echo "deb package contains files not owned by root:root" >&2
  dpkg-deb --contents "$DEB_FILE" | awk '$2 != "root/root"'
  exit 1
fi

echo "==> Building rpm package"
RPM_PAYLOAD="$PACKAGE_STAGE_DIR/rpm-payload"
RPM_TOPDIR="$PACKAGE_STAGE_DIR/rpmbuild"
mkdir -p "$RPM_PAYLOAD/opt/p2premote/resources" "$RPM_PAYLOAD/usr/lib/systemd/system" \
  "$RPM_PAYLOAD/usr/share/applications" \
  "$RPM_TOPDIR/BUILD" "$RPM_TOPDIR/BUILDROOT" "$RPM_TOPDIR/RPMS" "$RPM_TOPDIR/SOURCES" "$RPM_TOPDIR/SPECS" "$RPM_TOPDIR/SRPMS"
cp -a "$PKG_ROOT/resources/." "$RPM_PAYLOAD/opt/p2premote/resources/"
cp "$APP_DIR/packaging/linux/common/p2premote-service.service" "$RPM_PAYLOAD/usr/lib/systemd/system/p2premote-service.service"
cp "$APP_DIR/packaging/linux/common/p2premote-web.desktop" "$RPM_PAYLOAD/usr/share/applications/p2premote.desktop"
rpmbuild -bb "$APP_DIR/packaging/linux/rpm/p2premote.spec" \
  --target "$RPM_ARCH" \
  --define "_topdir $RPM_TOPDIR" \
  --define "pkg_version $VERSION" \
  --define "pkg_release 1.$GIT_COMMIT" \
  --define "pkg_arch $RPM_ARCH" \
  --define "payload_root $RPM_PAYLOAD"
RPM_FILE=$(find "$RPM_TOPDIR/RPMS" -type f -name '*.rpm' -print -quit)
if [ -z "$RPM_FILE" ]; then
  echo "rpmbuild completed without producing an rpm file" >&2
  exit 1
fi
cp "$RPM_FILE" "$DIST_DIR/p2premote-headless-${VERSION}-1.${GIT_COMMIT}.${RPM_ARCH}.rpm"

echo "==> Done"
echo "Artifacts: $DIST_DIR"
