#!/usr/bin/env bash

set -euo pipefail
umask 022

usage() {
  echo "Usage: $0 -o <output-path> [-a amd64]" >&2
}

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC_DIR="${P2PREMOTE_PUNCH_DIR:-$REPO_ROOT/../p2premote-punch}"
OUT_PATH=""
GOARCH_VALUE="${GOARCH:-amd64}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    -o)
      OUT_PATH="${2:-}"
      shift 2
      ;;
    -a)
      GOARCH_VALUE="${2:-}"
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

OUT_DIR="$(dirname "$OUT_PATH")"
mkdir -p "$OUT_DIR"
OUT_PATH="$(cd "$OUT_DIR" && pwd)/$(basename "$OUT_PATH")"

echo "==> Building p2premote-punch archive for linux/$GOARCH_VALUE"
(
  cd "$SRC_DIR"
  GOOS=linux GOARCH="$GOARCH_VALUE" CGO_ENABLED=1 \
    go build -buildmode=c-archive -ldflags "-s -w" -o "$OUT_PATH" ./punchffi
)

chmod 644 "$OUT_PATH"
HEADER_PATH="${OUT_PATH%.a}.h"
if [[ -f "$HEADER_PATH" ]]; then
  rm -f "$HEADER_PATH"
fi

echo "Built: $OUT_PATH"
