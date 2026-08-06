#!/usr/bin/env bash

set -euo pipefail
umask 022

VERSION="1.0.20260223"
SHA256="af459827b80bfd31b83b08077f4b5843acb7d18ad9a33a2ef532d3090f291fbf"
SOURCE_URL="https://git.zx2c4.com/wireguard-tools/snapshot/wireguard-tools-${VERSION}.tar.xz"

usage() {
  echo "Usage: $0 -o <output-path> -a <amd64|arm64>" >&2
}

OUT_PATH=""
ARCH=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o)
      if [[ $# -lt 2 || -z "$2" ]]; then
        echo "-o requires an output path" >&2
        exit 1
      fi
      OUT_PATH="$2"
      shift 2
      ;;
    -a)
      if [[ $# -lt 2 || -z "$2" ]]; then
        echo "-a requires amd64 or arm64" >&2
        exit 1
      fi
      ARCH="$2"
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

if [[ -z "$OUT_PATH" ]]; then
  usage
  exit 1
fi
if [[ -z "$ARCH" ]]; then
  echo "Linux architecture is required; pass -a amd64 or -a arm64." >&2
  exit 1
fi

case "$ARCH" in
  amd64)
    DEFAULT_CC="x86_64-linux-musl-gcc"
    ;;
  arm64)
    DEFAULT_CC="aarch64-linux-musl-gcc"
    ;;
  *)
    echo "Unsupported Linux architecture: $ARCH" >&2
    exit 1
    ;;
esac

CC_VALUE="${P2PREMOTE_WG_CC:-$DEFAULT_CC}"
if ! command -v "$CC_VALUE" >/dev/null 2>&1; then
  echo "Required compiler not found: $CC_VALUE" >&2
  exit 1
fi

CC_MACHINE="$("$CC_VALUE" -dumpmachine)"
case "$ARCH" in
  amd64)
    [[ "$CC_MACHINE" == x86_64-* ]] || {
      echo "Compiler $CC_VALUE targets $CC_MACHINE, expected x86_64." >&2
      exit 1
    }
    ;;
  arm64)
    [[ "$CC_MACHINE" == aarch64-* ]] || {
      echo "Compiler $CC_VALUE targets $CC_MACHINE, expected aarch64." >&2
      exit 1
    }
    ;;
esac

for tool in curl sha256sum tar make readelf; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Required build tool not found: $tool" >&2
    exit 1
  fi
done

OUT_DIR="$(dirname "$OUT_PATH")"
mkdir -p "$OUT_DIR"
OUT_PATH="$(cd "$OUT_DIR" && pwd)/$(basename "$OUT_PATH")"
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
ARCHIVE="$WORK_DIR/wireguard-tools.tar.xz"

echo "==> Downloading wireguard-tools v${VERSION}"
curl --fail --location --retry 3 --silent --show-error "$SOURCE_URL" -o "$ARCHIVE"
echo "$SHA256  $ARCHIVE" | sha256sum --check --status
mkdir -p "$WORK_DIR/source"
tar -xJf "$ARCHIVE" -C "$WORK_DIR/source" --strip-components=1

# Debian's musl-gcc does not put Linux UAPI headers in its default include tree.
# `-idirafter` keeps musl libc headers authoritative and only supplies missing
# linux/ and asm/ definitions from the host kernel headers.
EXTRA_CFLAGS=""
for include_dir in /usr/include "/usr/include/$($CC_VALUE -dumpmachine)"; do
  if [[ -d "$include_dir" ]]; then
    EXTRA_CFLAGS+=" -idirafter $include_dir"
  fi
done

echo "==> Building static wg for linux/${ARCH} with ${CC_VALUE}"
export CFLAGS="-O3${EXTRA_CFLAGS}"
make -C "$WORK_DIR/source/src" wg \
  CC="$CC_VALUE" \
  LDFLAGS="-static" \
  WITH_WGQUICK=no \
  WITH_BASHCOMPLETION=no \
  WITH_SYSTEMDUNITS=no

cp "$WORK_DIR/source/src/wg" "$OUT_PATH"
chmod 755 "$OUT_PATH"
if readelf -l "$OUT_PATH" | grep -q 'INTERP'; then
  echo "wg is dynamically linked; refusing non-portable package" >&2
  exit 1
fi
EXPECTED_MACHINE=""
case "$ARCH" in
  amd64) EXPECTED_MACHINE="Advanced Micro Devices X86-64" ;;
  arm64) EXPECTED_MACHINE="AArch64" ;;
esac
if ! readelf -h "$OUT_PATH" | grep -q "Machine:.*$EXPECTED_MACHINE"; then
  echo "wg architecture does not match linux/$ARCH" >&2
  exit 1
fi
"$OUT_PATH" --version
echo "Built: $OUT_PATH"
