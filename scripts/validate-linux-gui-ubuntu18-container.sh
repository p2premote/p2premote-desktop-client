#!/usr/bin/env bash
set -euo pipefail

deb="$1"
appimage="$2"
dpkg -i "$deb"
[[ -L /etc/systemd/system/multi-user.target.wants/p2premote-service.service ]] || {
  echo "p2premote service was not enabled during deb installation" >&2
  exit 1
}
[[ -f /usr/lib/systemd/system/p2premote-service.service ]] || {
  echo "p2premote systemd unit is missing" >&2
  exit 1
}

binaries=(
  /opt/p2premote/p2premote
  /opt/p2premote/resources/p2premote-service
  /opt/p2premote/resources/p2premote-cli
  /opt/p2premote/resources/wireguard-go
  /opt/p2premote/resources/wg
)
scripts/check-linux-abi.sh "${binaries[@]}"
for binary in "${binaries[@]}"; do
  if ldd "$binary" | grep -q 'not found'; then
    echo "missing runtime dependency for $binary" >&2
    ldd "$binary" >&2
    exit 1
  fi
done

set +e
timeout --signal=TERM 8s /opt/p2premote/resources/p2premote-service --foreground \
  >/tmp/p2premote-service.log 2>&1
service_status=$?
set -e
cat /tmp/p2premote-service.log
if [[ $service_status -ne 124 ]]; then
  echo "p2premote service exited before the smoke-test timeout (status $service_status)" >&2
  exit 1
fi
echo "Ubuntu 18.04 service smoke test OK"

run_gui_smoke_test() {
  local label="$1"
  shift
  set +e
  runuser -u p2ptest -- dbus-run-session -- timeout 12s xvfb-run -a "$@" >"/tmp/${label}.log" 2>&1
  local status=$?
  set -e
  cat "/tmp/${label}.log"
  if [[ $status -ne 124 ]]; then
    echo "$label exited before the smoke-test timeout (status $status)" >&2
    exit 1
  fi
  echo "Ubuntu 18.04 GUI smoke test OK: $label"
}

useradd --create-home --shell /bin/bash p2ptest
run_gui_smoke_test deb /usr/bin/p2premote
grep -Fq 'verified bundled file: /opt/p2premote/resources/p2premote-cli' /tmp/deb.log || {
  echo "deb GUI did not resolve its bundled CLI from /opt/p2premote/resources" >&2
  exit 1
}
run_gui_smoke_test appimage env APPIMAGE_EXTRACT_AND_RUN=1 "$appimage"
