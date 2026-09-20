#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || ! "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 <version>" >&2
  exit 2
fi

version="$1"
# Rust 编译缓存根目录（与 build-linux-gui-ubuntu18.sh 保持一致）
RUST_CACHE_ROOT="${RUST_CACHE_ROOT:-/mnt/n/rust-cache}"
cargo_target_dir="${CARGO_TARGET_DIR:-$RUST_CACHE_ROOT/wsl/linux-gui-x64}"
deb="$cargo_target_dir/release/bundle/deb/p2premote_${version}_amd64.deb"
stage="$(mktemp -d /tmp/p2premote-gui-deb.XXXXXX)"
output="${stage}/p2premote.deb"
cleanup() { rm -rf "$stage"; }
trap cleanup EXIT

[[ -f "$deb" ]] || { echo "missing deb: $deb" >&2; exit 1; }
dpkg-deb --raw-extract "$deb" "$stage/root"

install -d -m 0755 "$stage/root/opt/p2premote" "$stage/root/opt/p2premote/resources"
install -m 0755 "$stage/root/usr/bin/p2premote" "$stage/root/opt/p2premote/p2premote"
cp -a "$stage/root/usr/lib/p2premote/resources/." "$stage/root/opt/p2premote/resources/"
rm -rf "$stage/root/usr/lib/p2premote"
install -m 0755 packaging/linux/gui-ubuntu18/p2premote-launcher "$stage/root/usr/bin/p2premote"
install -m 0755 packaging/linux/common/configure-installation.sh \
  "$stage/root/opt/p2premote/resources/configure-installation"
install -d -m 0755 "$stage/root/usr/lib/systemd/system"
install -m 0644 packaging/linux/common/p2premote-service.service \
  "$stage/root/usr/lib/systemd/system/p2premote-service.service"

# Shared runtime state is writable, while application binaries remain root-owned and read-only.
install -d -m 1777 "$stage/root/opt/p2premote/data" "$stage/root/opt/p2premote/logs"
install -d -m 0755 "$stage/root/opt/p2premote/run"
if [[ -e "$stage/root/DEBIAN/postinst" ]]; then
  echo "unexpected existing deb postinst; refusing to overwrite it" >&2
  exit 1
fi
install -m 0755 packaging/linux/gui-ubuntu18/postinst "$stage/root/DEBIAN/postinst"
for script in preinst prerm postrm; do
  if [[ -e "$stage/root/DEBIAN/$script" ]]; then
    echo "unexpected existing deb $script; refusing to overwrite it" >&2
    exit 1
  fi
  install -m 0755 "packaging/linux/deb/$script" "$stage/root/DEBIAN/$script"
done

chown -R 0:0 "$stage/root"
dpkg-deb --root-owner-group --build "$stage/root" "$output"
install -m 0644 "$output" "$deb"
dpkg-deb --info "$deb" >/dev/null
echo "Repacked GUI deb with /opt/p2premote layout: $deb"
