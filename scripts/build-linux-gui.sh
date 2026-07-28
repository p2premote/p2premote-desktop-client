#!/usr/bin/env bash

set -euo pipefail

cat >&2 <<'EOF'
Linux GUI/AppImage packaging has been removed.

Linux now ships the headless service/CLI plus browser management UI:
  ./scripts/build-linux-headless.sh -v <version>

Linux headless packages use static musl binaries by default:
  ./scripts/build-linux-headless.sh -v <version>
EOF

exit 1
