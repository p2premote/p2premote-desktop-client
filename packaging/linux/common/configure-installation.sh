#!/bin/sh
set -eu

INSTALL_ROOT="/opt/p2premote"
CONFIG_PATH="$INSTALL_ROOT/data/config.json"
CLI="$INSTALL_ROOT/resources/p2premote-cli"

if [ "$(id -u)" != "0" ]; then
  echo "configure-installation must be run as root" >&2
  exit 1
fi

mkdir -p "$INSTALL_ROOT/data" "$INSTALL_ROOT/logs" "$INSTALL_ROOT/run"
chmod 700 "$INSTALL_ROOT/data"

if [ ! -e "$CONFIG_PATH" ]; then
  cat > "$CONFIG_PATH" <<'EOF'
{
  "web_admin_security_code": "0000",
  "web_admin_security_code_must_change": true
}
EOF
  chmod 600 "$CONFIG_PATH"
  echo "Default Web security code: 0000 (must be changed on first sign-in)"
elif ! grep -Eq '"web_admin_security_code"[[:space:]]*:[[:space:]]*"[^"]+"' "$CONFIG_PATH"; then
  if [ ! -x "$CLI" ]; then
    echo "missing required executable: $CLI" >&2
    exit 1
  fi
  "$CLI" config set web_admin_security_code 0000
  "$CLI" config set web_admin_security_code_must_change true
  chmod 600 "$CONFIG_PATH"
  echo "Default Web security code: 0000 (must be changed on first sign-in)"
fi
