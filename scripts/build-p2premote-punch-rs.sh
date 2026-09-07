#!/usr/bin/env bash

# Build the Rust rewrite (p2premote-punch-rs) of the punch FFI as a static
# musl library. The Rust artifact replaces the Go c-archive, whose runtime
# could not be embedded into a musl host without crashes.

set -euo pipefail
umask 022

usage() {
  echo "Usage: $0 -o <output-path> -a <amd64|arm64> -s <p2premote-punch-rs-dir>" >&2
}

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC_DIR=""
OUT_PATH=""
ARCH_VALUE=""

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
    -s)
      if [[ $# -lt 2 || -z "$2" ]]; then
        echo "-s requires the p2premote-punch-rs directory" >&2
        exit 1
      fi
      SRC_DIR="$2"
      shift 2
      ;;
    -a)
      if [[ $# -lt 2 || -z "$2" ]]; then
        echo "-a requires amd64 or arm64" >&2
        exit 1
      fi
      ARCH_VALUE="$2"
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
if [[ -z "$SRC_DIR" ]]; then
  echo "p2premote-punch-rs directory is required; pass -s <path>." >&2
  exit 1
fi
if [[ -z "$ARCH_VALUE" ]]; then
  echo "Linux architecture is required; pass -a amd64 or -a arm64." >&2
  exit 1
fi
case "$ARCH_VALUE" in
  amd64) RUST_TARGET="x86_64-unknown-linux-musl" ;;
  arm64) RUST_TARGET="aarch64-unknown-linux-musl" ;;
  *)
    echo "Unsupported Linux architecture: $ARCH_VALUE" >&2
    exit 1
    ;;
esac

OUT_DIR="$(dirname "$OUT_PATH")"
mkdir -p "$OUT_DIR"
OUT_PATH="$(cd "$OUT_DIR" && pwd)/$(basename "$OUT_PATH")"

if ! (cd "$SRC_DIR" && rustup target list --installed 2>/dev/null | grep -qx "$RUST_TARGET"); then
  echo "Rust std for $RUST_TARGET is not installed; run: rustup target add $RUST_TARGET" >&2
  exit 1
fi

echo "==> Building p2premote-punch static library for linux/$ARCH_VALUE (Rust, $RUST_TARGET)"
(
  cd "$SRC_DIR"
  # plain cargo build: overriding --crate-type via cargo rustc has produced
  # archives whose bundled std members did not match their references.
  # The crate pins its toolchain via rust-toolchain.toml (same 1.94.1 as the
  # client) — the stripped archive requires the host to use that same std.
  cargo build --release --lib --target "$RUST_TARGET"
)

BUILT="$SRC_DIR/target/$RUST_TARGET/release/libp2premote_punch.a"
if [[ ! -f "$BUILT" ]]; then
  echo "build finished but the static library is missing: $BUILT" >&2
  exit 1
fi
cp "$BUILT" "$OUT_PATH"

# The Rust host already links its own std (and ring/compiler-rt); the bundled
# rustlib members in a rustc staticlib would collide with them. Strip them
# from the delivered copy (the in-tree build stays complete for C hosts).
"$SRC_DIR/scripts/strip-rustlib.sh" "$OUT_PATH" "$RUST_TARGET"

chmod 644 "$OUT_PATH"

echo "Built: $OUT_PATH"
