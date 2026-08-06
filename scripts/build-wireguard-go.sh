#!/usr/bin/env bash

set -euo pipefail
umask 022

usage() {
  echo "Usage: $0 -o <output-path> -a <amd64|arm64> -s <p2premote-punch-dir>" >&2
}

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC_DIR=""
OUT_PATH=""
GOARCH_VALUE=""

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
        echo "-s requires the p2premote-punch directory" >&2
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
      GOARCH_VALUE="$2"
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
  echo "p2premote-punch directory is required; pass -s <path>." >&2
  exit 1
fi
if [[ -z "$GOARCH_VALUE" ]]; then
  echo "Linux architecture is required; pass -a amd64 or -a arm64." >&2
  exit 1
fi
case "$GOARCH_VALUE" in
  amd64|arm64) ;;
  *)
    echo "Unsupported Linux architecture: $GOARCH_VALUE" >&2
    exit 1
    ;;
esac

OUT_DIR="$(dirname "$OUT_PATH")"
mkdir -p "$OUT_DIR"
OUT_PATH="$(cd "$OUT_DIR" && pwd)/$(basename "$OUT_PATH")"

echo "==> Building wireguard-go for linux/$GOARCH_VALUE"
(
  cd "$SRC_DIR"
  GOOS=linux GOARCH="$GOARCH_VALUE" CGO_ENABLED=0 \
    go build -trimpath -ldflags "-s -w" -o "$OUT_PATH" github.com/tailscale/wireguard-go
)
chmod 755 "$OUT_PATH"
echo "Built: $OUT_PATH"
