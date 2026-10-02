#!/usr/bin/env bash

# Shared Linux prefix layout used by native headless packages and Docker.
# Callers compile with their own P2PREMOTE_RELEASE_TARGET, then pass the
# resulting binaries here so delivery formats cannot drift apart.
stage_linux_prefix() {
  local root="$1"
  local release_dir="$2"
  local wireguard_go="$3"
  local wg_cli="$4"
  local app_dir="$5"

  mkdir -p "$root/resources/web"
  cp "$release_dir/p2premote-service" "$root/resources/p2premote-service"
  cp "$release_dir/p2premote-cli" "$root/resources/p2premote-cli"
  cp "$wireguard_go" "$root/resources/wireguard-go"
  cp "$wg_cli" "$root/resources/wg"
  cp "$app_dir/src-tauri/resources/.p2premote_default.json" "$root/resources/.p2premote_default.json"
  cp "$app_dir/packaging/linux/common/configure-installation.sh" "$root/resources/configure-installation"
  cp "$app_dir/packaging/linux/common/configure-kysec.sh" "$root/resources/configure-kysec"
  cp -a "$app_dir/dist/." "$root/resources/web/"
  cp "$app_dir/packaging/linux/common/p2premote-service.service" "$root/p2premote-service.service"
  cp "$app_dir/packaging/linux/common/p2premote-web.desktop" "$root/p2premote-web.desktop"
  cp "$app_dir/src-tauri/icons/icon.png" "$root/p2premote.png"
  cp "$app_dir/packaging/linux/headless/scripts/install-service.sh" "$root/install-service.sh"
  cp "$app_dir/packaging/linux/headless/scripts/install-gui.sh" "$root/install-gui.sh"
  cp "$app_dir/packaging/linux/headless/scripts/install-p2premote.desktop" "$root/安装-p2pRemote.desktop"
  cp "$app_dir/packaging/linux/headless/scripts/uninstall-service.sh" "$root/uninstall-service.sh"

  chmod 755 \
    "$root/resources/p2premote-service" \
    "$root/resources/p2premote-cli" \
    "$root/resources/wireguard-go" \
    "$root/resources/wg" \
    "$root/resources/configure-installation" \
    "$root/resources/configure-kysec" \
    "$root/install-service.sh" \
    "$root/install-gui.sh" \
    "$root/安装-p2pRemote.desktop" \
    "$root/uninstall-service.sh"
  chmod 644 \
    "$root/resources/.p2premote_default.json" \
    "$root/p2premote-service.service" \
    "$root/p2premote-web.desktop" \
    "$root/p2premote.png"
  find "$root/resources/web" -type d -exec chmod 755 {} +
  find "$root/resources/web" -type f -exec chmod 644 {} +
}
