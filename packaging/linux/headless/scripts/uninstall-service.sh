#!/bin/sh
set -eu

INSTALL_ROOT="/opt/p2premote"
SERVICE_NAME="p2premote-service"
UNIT_PATH="/etc/systemd/system/${SERVICE_NAME}.service"
PACKAGE_UNIT_PATH="/usr/lib/systemd/system/${SERVICE_NAME}.service"
DESKTOP_PATH="/usr/share/applications/p2premote.desktop"
ICON_PATH="/usr/share/icons/hicolor/512x512/apps/p2premote.png"

if [ "$(id -u)" != "0" ]; then
  echo "uninstall-service.sh must be run as root" >&2
  exit 1
fi

if command -v systemctl >/dev/null 2>&1; then
  systemctl stop "$SERVICE_NAME" >/dev/null 2>&1 || true
  systemctl disable "$SERVICE_NAME" >/dev/null 2>&1 || true
fi

if [ -x "$INSTALL_ROOT/resources/configure-kysec" ]; then
  "$INSTALL_ROOT/resources/configure-kysec" remove || true
fi

rm -f "$UNIT_PATH"
rm -f "$PACKAGE_UNIT_PATH"
rm -f "$DESKTOP_PATH"
rm -f "$ICON_PATH"
rm -rf "$INSTALL_ROOT"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$(dirname "$DESKTOP_PATH")" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload >/dev/null 2>&1 || true
  systemctl reset-failed "$SERVICE_NAME" >/dev/null 2>&1 || true
fi

echo "p2pRemote service uninstalled."
