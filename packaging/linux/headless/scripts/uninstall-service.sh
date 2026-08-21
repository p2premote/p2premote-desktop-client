#!/bin/sh
set -eu

INSTALL_ROOT="/opt/p2premote"
SERVICE_NAME="p2premote-service"
UNIT_PATH="/etc/systemd/system/${SERVICE_NAME}.service"
PACKAGE_UNIT_PATH="/usr/lib/systemd/system/${SERVICE_NAME}.service"

if [ "$(id -u)" != "0" ]; then
  echo "uninstall-service.sh must be run as root" >&2
  exit 1
fi

if command -v systemctl >/dev/null 2>&1; then
  systemctl stop "$SERVICE_NAME" >/dev/null 2>&1 || true
  systemctl disable "$SERVICE_NAME" >/dev/null 2>&1 || true
fi

rm -f "$UNIT_PATH"
rm -f "$PACKAGE_UNIT_PATH"
rm -rf "$INSTALL_ROOT"

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload >/dev/null 2>&1 || true
  systemctl reset-failed "$SERVICE_NAME" >/dev/null 2>&1 || true
fi

echo "p2pRemote service uninstalled."
