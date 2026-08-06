#!/usr/bin/env bash

# 构建 p2premote Linux 客户端 Docker 镜像。
#
# 流程：
#   1) 调用 build-linux-headless.sh 生成当前 Linux 主机架构的 GNU/glibc tgz
#   2) 将 tgz 拷贝到 packaging/linux/docker/ 下并重命名为
#      p2premote-headless.tar.gz（Dockerfile 默认 ARG）
#   3) docker build -t <tag> -f Dockerfile packaging/linux/docker/
#   4) 将镜像保存为带版本号的 tar 包
#   5) 清理 docker context 内的临时 tgz
#
# 用法:
#   ./scripts/build-docker.sh -v <version> [--tag <docker-tag>] [--no-tgz-build]
#
#   -v <version>        客户端版本号，传给 build-linux-headless.sh
#   --tag <docker-tag>  自定义镜像 tag，默认 p2premote/client:<version>
#   --no-tgz-build      跳过 headless 构建，假定 dist 目录下已存在 tgz
#                       （需配合 --tgz-path 或自动按版本探测）

set -euo pipefail
umask 022

usage() {
    cat >&2 <<'EOF'
Usage: build-docker.sh -v <version> [--tag <docker-tag>] [--no-tgz-build]
                      [--tgz-path <path>] [-h]
EOF
}

VERSION=""
DOCKER_TAG=""
SKIP_TGZ_BUILD=0
EXPLICIT_TGZ=""

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
        --tag)
            if [[ $# -lt 2 || -z "$2" ]]; then
                echo "--tag requires a value" >&2
                exit 1
            fi
            DOCKER_TAG="$2"
            shift 2
            ;;
        --no-tgz-build)
            SKIP_TGZ_BUILD=1
            shift
            ;;
        --tgz-path)
            if [[ $# -lt 2 || -z "$2" ]]; then
                echo "--tgz-path requires a file path" >&2
                exit 1
            fi
            EXPLICIT_TGZ="$2"
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

if [[ "$(uname -s)" != "Linux" ]]; then
    echo "This script must run in a Linux or WSL shell." >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
APP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST_DIR="$APP_DIR/build/linux/dist/headless"
DOCKER_DIST_DIR="$APP_DIR/build/linux/dist/docker"
DOCKER_CTX="$APP_DIR/packaging/linux/docker"
DOCKERFILE="$DOCKER_CTX/Dockerfile"
DEFAULT_TGZ_NAME="p2premote-headless.tar.gz"

if [[ -z "$DOCKER_TAG" ]]; then
    DOCKER_TAG="p2premote/client:${VERSION}"
fi

if [[ ! -f "$DOCKERFILE" ]]; then
    echo "Dockerfile not found: $DOCKERFILE" >&2
    exit 1
fi

if [[ "$SKIP_TGZ_BUILD" -eq 0 ]]; then
    echo "==> Building headless tgz via build-linux-headless.sh"
    HEADLESS_ARGS=(-v "$VERSION")
    "$SCRIPT_DIR/build-linux-headless.sh" "${HEADLESS_ARGS[@]}"
else
    echo "==> Skipping headless build (--no-tgz-build)"
fi

# 选择 dist 目录中的 tgz：显式路径和当前 Linux 主机架构都必须严格匹配。
case "$(uname -m)" in
    aarch64) TARGET_LABEL="aarch64-linux-gnu" ;;
    x86_64) TARGET_LABEL="x86_64-linux-gnu" ;;
    *) echo "Unsupported Linux host architecture: $(uname -m)" >&2; exit 1 ;;
esac
if [[ -n "$EXPLICIT_TGZ" ]]; then
    TGZ_PATH="$EXPLICIT_TGZ"
else
    shopt -s nullglob
    TGZ_CANDIDATES=("$DIST_DIR"/p2premote-headless_"$VERSION"-*_"$TARGET_LABEL".tar.gz)
    shopt -u nullglob
    if (( ${#TGZ_CANDIDATES[@]} != 1 )); then
        echo "Expected exactly one ${TARGET_LABEL} headless tgz for version $VERSION under $DIST_DIR; found ${#TGZ_CANDIDATES[@]}." >&2
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
    echo "run without --no-tgz-build, or pass --tgz-path <path>" >&2
    exit 1
fi

echo "==> Using headless tgz: $TGZ_PATH"
echo "==> Preparing docker context: $DOCKER_CTX"
CONTEXT_TGZ="$DOCKER_CTX/$DEFAULT_TGZ_NAME"
IMAGE_TAR="$DOCKER_DIST_DIR/p2premote-client_${VERSION}.tar"
IMAGE_TAR_TMP="${IMAGE_TAR}.tmp"
cleanup() {
    rm -f "$CONTEXT_TGZ" "$IMAGE_TAR_TMP"
}
trap cleanup EXIT
cp -f "$TGZ_PATH" "$CONTEXT_TGZ"

echo "==> Building docker image: $DOCKER_TAG"
docker build \
    -t "$DOCKER_TAG" \
    -f "$DOCKERFILE" \
    --build-arg "P2P_HEADLESS_TGZ=$DEFAULT_TGZ_NAME" \
    "$DOCKER_CTX"

echo "==> Saving docker image: $IMAGE_TAR"
mkdir -p "$DOCKER_DIST_DIR"
docker save -o "$IMAGE_TAR_TMP" "$DOCKER_TAG"
mv -f "$IMAGE_TAR_TMP" "$IMAGE_TAR"

echo "==> Done"
echo "Image: $DOCKER_TAG"
echo "Image tar: $IMAGE_TAR"
echo "Run (host network):"
echo "  docker run -d --name p2premote \\"
echo "    --network host \\"
echo "    --cap-add NET_ADMIN --cap-add NET_RAW \\"
echo "    --device /dev/net/tun \\"
echo "    -v /opt/p2premote/data:/opt/p2premote/data \\"
echo "    -v /opt/p2premote/logs:/opt/p2premote/logs \\"
echo "    --restart unless-stopped \\"
echo "    $DOCKER_TAG"
