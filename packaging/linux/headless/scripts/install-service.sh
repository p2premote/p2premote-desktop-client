#!/bin/sh
set -eu

INSTALL_ROOT="/opt/p2premote"
SERVICE_NAME="p2premote-service"
UNIT_PATH="/etc/systemd/system/${SERVICE_NAME}.service"

if [ "$(id -u)" != "0" ]; then
  echo "install-service.sh must be run as root" >&2
  exit 1
fi

if ! command -v systemctl >/dev/null 2>&1; then
  echo "systemctl is required" >&2
  exit 1
fi

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

for required_file in \
  p2premote-service \
  p2premote-cli \
  wireguard-go \
  wg \
  configure-installation \
  .p2premote_default.json; do
  if [ ! -f "$SCRIPT_DIR/resources/$required_file" ]; then
    echo "missing required file: $SCRIPT_DIR/resources/$required_file" >&2
    exit 1
  fi
done

if [ ! -f "$SCRIPT_DIR/p2premote-service.service" ]; then
  echo "missing required file: $SCRIPT_DIR/p2premote-service.service" >&2
  exit 1
fi

if [ ! -d "$SCRIPT_DIR/resources/web" ]; then
  echo "missing required directory: $SCRIPT_DIR/resources/web" >&2
  exit 1
fi

mkdir -p "$INSTALL_ROOT/resources" "$INSTALL_ROOT/systemd"

copy_file() {
  src="$1"
  dst="$2"
  mode="$3"
  if [ ! -f "$src" ]; then
    echo "missing required file: $src" >&2
    exit 1
  fi
  cp "$src" "$dst"
  chmod "$mode" "$dst"
}

if systemctl is-active --quiet "$SERVICE_NAME"; then
  echo "Stopping running ${SERVICE_NAME} before installation..."
  systemctl stop "$SERVICE_NAME"
fi

copy_file "$SCRIPT_DIR/resources/p2premote-service" "$INSTALL_ROOT/resources/p2premote-service" 755
copy_file "$SCRIPT_DIR/resources/p2premote-cli" "$INSTALL_ROOT/resources/p2premote-cli" 755
copy_file "$SCRIPT_DIR/resources/wireguard-go" "$INSTALL_ROOT/resources/wireguard-go" 755
copy_file "$SCRIPT_DIR/resources/wg" "$INSTALL_ROOT/resources/wg" 755
copy_file "$SCRIPT_DIR/resources/configure-installation" "$INSTALL_ROOT/resources/configure-installation" 755
copy_file "$SCRIPT_DIR/resources/.p2premote_default.json" "$INSTALL_ROOT/resources/.p2premote_default.json" 644

CONFIGURE_OUTPUT=$("$INSTALL_ROOT/resources/configure-installation")

rm -rf "$INSTALL_ROOT/resources/web"
mkdir -p "$INSTALL_ROOT/resources/web"
cp -a "$SCRIPT_DIR/resources/web/." "$INSTALL_ROOT/resources/web/"
chmod -R a+rX "$INSTALL_ROOT/resources/web"
copy_file "$SCRIPT_DIR/p2premote-service.service" "$INSTALL_ROOT/systemd/${SERVICE_NAME}.service" 644

cp "$INSTALL_ROOT/systemd/${SERVICE_NAME}.service" "$UNIT_PATH"
systemctl daemon-reload
systemctl enable "$SERVICE_NAME"
systemctl restart "$SERVICE_NAME"

echo "p2pRemote service installed and started."
echo "Web UI: http://127.0.0.1:48083/"
if [ -n "$CONFIGURE_OUTPUT" ]; then
  printf '%s\n' "$CONFIGURE_OUTPUT"
fi
