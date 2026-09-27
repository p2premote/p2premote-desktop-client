#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || ! "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 <version>" >&2
  exit 2
fi

version="$1"
# Rust 编译缓存根目录（与 build-linux-gui.sh 保持一致）
RUST_CACHE_ROOT="${RUST_CACHE_ROOT:-/mnt/n/rust-cache}"
cargo_target_dir="${CARGO_TARGET_DIR:-$RUST_CACHE_ROOT/wsl/linux-gui-x64}"
deb="$cargo_target_dir/release/bundle/deb/p2premote_${version}_amd64.deb"
tiny_deb="${RUSTDESK_TINY_DEB:-src-tauri/resources/RustDeskTiny.deb}"
stage="$(mktemp -d /tmp/p2premote-gui-deb.XXXXXX)"
output="${stage}/p2premote.deb"
cleanup() { rm -rf "$stage"; }
trap cleanup EXIT

[[ -f "$deb" ]] || { echo "missing deb: $deb" >&2; exit 1; }
[[ -f "$tiny_deb" ]] || { echo "missing RustDeskTiny deb: $tiny_deb" >&2; exit 1; }
dpkg-deb --raw-extract "$deb" "$stage/root"

extract_deb_member() {
  local archive="$1"
  local prefix="$2"
  local destination="$3"
  local member
  member="$(ar t "$archive" | awk -v prefix="$prefix" 'index($0, prefix ".tar") == 1 { print; exit }')"
  [[ -n "$member" ]] || { echo "missing $prefix archive in $archive" >&2; exit 1; }
  mkdir -p "$destination"
  case "$member" in
    *.zst) ar p "$archive" "$member" | zstd -dc | tar -xf - -C "$destination" ;;
    *.xz) ar p "$archive" "$member" | tar -xJf - -C "$destination" ;;
    *.gz) ar p "$archive" "$member" | tar -xzf - -C "$destination" ;;
    *) echo "unsupported deb member compression: $member" >&2; exit 1 ;;
  esac
}

# Debian 10's dpkg predates .deb zstd support. Extract the upstream package's
# ar members explicitly so the compile baseline stays Debian 10.
extract_deb_member "$tiny_deb" control "$stage/tiny-control"
extract_deb_member "$tiny_deb" data "$stage/tiny"
tiny_control="$stage/tiny-control/control"
tiny_arch="$(sed -n 's/^Architecture: //p' "$tiny_control")"
[[ "$tiny_arch" == amd64 ]] || { echo "RustDeskTiny deb must be amd64 (found $tiny_arch)" >&2; exit 1; }

check_tiny_abi() {
  local binary="$1"
  local family="$2"
  local maximum="$3"
  local required
  while IFS= read -r required; do
    [[ -z "$required" ]] && continue
    if [[ "$(printf '%s\n%s\n' "$maximum" "$required" | sort -V | tail -n1)" != "$maximum" ]]; then
      echo "RustDeskTiny is not compatible with the Debian 10 baseline: $binary requires ${family}_${required} (maximum ${family}_${maximum})" >&2
      exit 1
    fi
  done < <(readelf --version-info "$binary" 2>/dev/null \
    | grep -oE "${family}_[0-9]+(\\.[0-9]+)+" \
    | sed "s/${family}_//" | sort -Vu || true)
}
while IFS= read -r -d '' tiny_file; do
  file "$tiny_file" | grep -q 'ELF ' || continue
  check_tiny_abi "$tiny_file" GLIBC 2.28
  check_tiny_abi "$tiny_file" GLIBCXX 3.4.25
done < <(find "$stage/tiny" -type f -print0)

if [[ -e "$stage/tiny/usr/bin/p2premote" || -e "$stage/tiny/opt/p2premote" ]]; then
  echo "RustDeskTiny deb collides with p2premote paths" >&2
  exit 1
fi
cp -a "$stage/tiny/." "$stage/root/"

install -d -m 0755 "$stage/root/opt/p2premote" "$stage/root/opt/p2premote/resources"
install -m 0755 "$stage/root/usr/bin/p2premote" "$stage/root/opt/p2premote/p2premote"
cp -a "$stage/root/usr/lib/p2premote/resources/." "$stage/root/opt/p2premote/resources/"
rm -rf "$stage/root/usr/lib/p2premote"
install -m 0755 packaging/linux/gui-ubuntu18/p2premote-launcher "$stage/root/usr/bin/p2premote"
install -m 0755 packaging/linux/common/configure-installation.sh \
  "$stage/root/opt/p2premote/resources/configure-installation"
install -d -m 0755 "$stage/root/lib/systemd/system"
install -m 0644 packaging/linux/common/p2premote-service.service \
  "$stage/root/lib/systemd/system/p2premote-service.service"

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

# RustDeskTiny is now part of this package, so carry its runtime dependencies
# into the combined control file. Alternative dependencies keep old Kylin and
# new Debian/Ubuntu package names compatible.
tiny_depends="$(sed -n 's/^Depends: //p' "$tiny_control")"
current_depends="$(sed -n 's/^Depends: //p' "$stage/root/DEBIAN/control")"
sed -i "s@^Depends: .*@Depends: ${current_depends}, ${tiny_depends}@" "$stage/root/DEBIAN/control"

# Reproduce the upstream deb's required service setup without installing a
# second package or copying its DEBIAN scripts over ours.
sed -i '/^exit 0$/d' "$stage/root/DEBIAN/postinst"
cat >> "$stage/root/DEBIAN/postinst" <<'EOF'
if [ "$1" = "configure" ]; then
  ln -f -s /usr/share/rustdesktiny/rustdesktiny /usr/bin/rustdesktiny
  if [ -d /run/systemd/system ]; then
    install -m 0644 /usr/share/rustdesktiny/files/systemd/rustdesk.service /usr/lib/systemd/system/rustdesktiny.service
    command -v pkill >/dev/null 2>&1 && sed -i 's|pkill|/usr/bin/pkill|g' /usr/lib/systemd/system/rustdesktiny.service
    systemctl daemon-reload
    systemctl enable rustdesktiny.service >/dev/null
    systemctl restart rustdesktiny.service
  fi
fi
exit 0
EOF
sed -i '/^exit 0$/d' "$stage/root/DEBIAN/prerm"
cat >> "$stage/root/DEBIAN/prerm" <<'EOF'
if [ "$1" = "remove" ] || [ "$1" = "upgrade" ]; then
  rm -f /usr/bin/rustdesktiny
  if [ -d /run/systemd/system ]; then
    systemctl stop rustdesktiny.service >/dev/null 2>&1 || true
    [ "$1" != "remove" ] || systemctl disable rustdesktiny.service >/dev/null 2>&1 || true
  fi
fi
exit 0
EOF

chown -R 0:0 "$stage/root"
dpkg-deb --root-owner-group --build "$stage/root" "$output"
install -m 0644 "$output" "$deb"
dpkg-deb --info "$deb" >/dev/null
dpkg-deb --contents "$deb" | grep -q './usr/share/rustdesktiny/rustdesktiny' || {
  echo "combined deb is missing RustDeskTiny" >&2
  exit 1
}
echo "Repacked GUI deb with /opt/p2premote layout: $deb"
