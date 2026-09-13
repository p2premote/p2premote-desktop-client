#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <headless-deb>" >&2
  exit 2
fi

deb="$1"
[[ -f "$deb" ]] || { echo "headless deb is missing: $deb" >&2; exit 1; }

# The validation container does not run systemd as PID 1. Keep maintainer
# scripts intact while replacing systemctl only inside this disposable image.
ln -sf /bin/true /usr/local/bin/systemctl
export PATH="/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
dpkg --ignore-depends=xdg-utils -i "$deb"

desktop=/usr/share/applications/p2premote.desktop
icon=/usr/share/icons/hicolor/512x512/apps/p2premote.png
grep -Fxq 'Exec=xdg-open http://127.0.0.1:48083' "$desktop"
[[ "$(stat -c '%a' "$desktop")" == 644 ]]
[[ "$(stat -c '%a' "$icon")" == 644 ]]

if find /opt/p2premote \( -iname '*rustdesk*' -o -name 'p2premote-desktop-engine' \) \
  -print -quit | grep -q .; then
  echo "Linux headless package contains a disabled remote desktop artifact" >&2
  exit 1
fi

set +e
timeout --signal=TERM 8s /opt/p2premote/resources/p2premote-service --foreground \
  >/tmp/p2premote-headless-service.log 2>&1
status=$?
set -e
cat /tmp/p2premote-headless-service.log
if [[ $status -ne 124 ]]; then
  echo "headless service exited before the smoke-test timeout (status $status)" >&2
  exit 1
fi

echo "Ubuntu 18.04 headless package smoke test OK"
