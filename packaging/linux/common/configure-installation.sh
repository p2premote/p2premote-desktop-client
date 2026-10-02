#!/bin/sh
set -eu

INSTALL_ROOT="/opt/p2premote"

if [ "$(id -u)" != "0" ]; then
  echo "configure-installation must be run as root" >&2
  exit 1
fi

# 仅准备目录与权限；不写任何初始配置。全新安装的 Web 管理走首次信任
# (TOFU) 引导：由首个浏览器客户端设置安全码与允许访问的 IP。
mkdir -p "$INSTALL_ROOT/data" "$INSTALL_ROOT/logs" "$INSTALL_ROOT/run"
chmod 700 "$INSTALL_ROOT/data"
