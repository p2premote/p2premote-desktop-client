#!/usr/bin/env bash
set -euo pipefail
if [[ $# -lt 1 ]]; then echo "usage: $0 <ELF> [ELF...]" >&2; exit 2; fi
for binary in "$@"; do
  file "$binary" | grep -q 'ELF 64-bit' || { echo "$binary is not a 64-bit ELF" >&2; exit 1; }
  if readelf --version-info "$binary" | grep -E 'GLIBC_2\.(29|[3-9][0-9])' >/dev/null; then
    echo "$binary requires glibc newer than Debian 10 (2.28)" >&2
    exit 1
  fi
  echo "ABI baseline OK: $binary"
done
