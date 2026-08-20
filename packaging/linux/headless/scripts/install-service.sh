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
CONFIG_PATH="$INSTALL_ROOT/data/config.json"
SHOW_DEFAULT_SECURITY_CODE=0
INITIALIZE_DEFAULT_SECURITY_CODE=0

for required_file in \
  p2premote-service \
  p2premote-cli \
  wireguard-go \
  wg \
  .p2premote_default.json; do
  if [ ! -f "$SCRIPT_DIR/resources/$required_file" ]; then
    echo "missing required file: $SCRIPT_DIR/resources/$required_file" >&2
    exit 1
  fi
done

if [ ! -d "$SCRIPT_DIR/resources/web" ]; then
  echo "missing required directory: $SCRIPT_DIR/resources/web" >&2
  exit 1
fi

mkdir -p "$INSTALL_ROOT/resources" "$INSTALL_ROOT/data" "$INSTALL_ROOT/logs" "$INSTALL_ROOT/run" "$INSTALL_ROOT/systemd"

if [ ! -e "$CONFIG_PATH" ]; then
  cat > "$CONFIG_PATH" <<'EOF'
{
  "web_admin_security_code": "0000",
  "web_admin_security_code_must_change": true
}
EOF
  chmod 600 "$CONFIG_PATH"
  SHOW_DEFAULT_SECURITY_CODE=1
elif ! grep -Eq '"web_admin_security_code"[[:space:]]*:[[:space:]]*"[^"]+"' "$CONFIG_PATH"; then
  INITIALIZE_DEFAULT_SECURITY_CODE=1
fi

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
copy_file "$SCRIPT_DIR/resources/.p2premote_default.json" "$INSTALL_ROOT/resources/.p2premote_default.json" 644

if [ "$INITIALIZE_DEFAULT_SECURITY_CODE" = "1" ]; then
  "$INSTALL_ROOT/resources/p2premote-cli" config set web_admin_security_code 0000
  "$INSTALL_ROOT/resources/p2premote-cli" config set web_admin_security_code_must_change true
  SHOW_DEFAULT_SECURITY_CODE=1
fi

rm -rf "$INSTALL_ROOT/resources/web"
mkdir -p "$INSTALL_ROOT/resources/web"
cp -a "$SCRIPT_DIR/resources/web/." "$INSTALL_ROOT/resources/web/"
chmod -R a+rX "$INSTALL_ROOT/resources/web"
chmod 700 "$INSTALL_ROOT/data"

cat > "$INSTALL_ROOT/systemd/${SERVICE_NAME}.service" <<'EOF'
[Unit]
Description=p2pRemote Service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/opt/p2premote/resources/p2premote-service --foreground
WorkingDirectory=/opt/p2premote
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF

cp "$INSTALL_ROOT/systemd/${SERVICE_NAME}.service" "$UNIT_PATH"
systemctl daemon-reload
systemctl enable "$SERVICE_NAME"
systemctl restart "$SERVICE_NAME"

echo "p2pRemote service installed and started."
echo "Web UI: http://127.0.0.1:48083/"
if [ "$SHOW_DEFAULT_SECURITY_CODE" = "1" ]; then
  echo "Default Web security code: 0000 (must be changed on first sign-in)"
fi
