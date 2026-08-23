#!/bin/sh

# Register p2pRemote with Kylin's application network-control policy.
# Do not edit /etc/kysec/netctl/*.xml directly: kysec-daemon owns those files.
# On Debian/Ubuntu this script is intentionally a no-op.

set -u

SERVICE_PATH="/opt/p2premote/resources/p2premote-service"
SERVICE_NAME="p2premote-headless"
DESKTOP_PATH="/usr/share/applications/p2premote.desktop"
KYSEC_BUS="com.kylin.kysec"
KYSEC_OBJECT="/netctl"
KYSEC_INTERFACE="com.kylin.kysec.netctl"

log() {
  echo "p2pRemote KYSEC: $*" >&2
}

is_kysec_available() {
  if [ ! -f /etc/kysec/kysec.conf ] && [ ! -f /etc/kylin-security.conf ]; then
    return 1
  fi
  command -v busctl >/dev/null 2>&1 || return 1
  busctl --system introspect "$KYSEC_BUS" "$KYSEC_OBJECT" \
    "$KYSEC_INTERFACE" >/dev/null 2>&1
}

# busctl exits successfully when the D-Bus call succeeds, even when the
# method returns a non-zero integer. KYSEC methods return i, so check it.
kysec_call() {
  method="$1"
  signature="$2"
  shift 2
  output=$(busctl --system call \
    "$KYSEC_BUS" "$KYSEC_OBJECT" "$KYSEC_INTERFACE" \
    "$method" "$signature" "$@" 2>/dev/null) || return 1
  case "$output" in
    "i 0"|"i 0 "*) return 0 ;;
    *) return 1 ;;
  esac
}

register_program() {
  desktop_path="$DESKTOP_PATH"
  [ -f "$desktop_path" ] || desktop_path=""

  if kysec_call kysec_netctl_ext_update usisssi \
    0 "$SERVICE_PATH" 1 "$SERVICE_NAME" "" "$desktop_path" 3; then
    return 0
  fi
  kysec_call kysec_netctl_ext_add usisssi \
    0 "$SERVICE_PATH" 1 "$SERVICE_NAME" "" "$desktop_path" 3
}

register_package() {
  if kysec_call kysec_netctl_pkg_update usissi \
    0 "$SERVICE_NAME" 1 "" "" 3; then
    return 0
  fi
  kysec_call kysec_netctl_pkg_add usissi \
    0 "$SERVICE_NAME" 1 "" "" 3
}

remove_program() {
  kysec_call kysec_netctl_remove us 0 "$SERVICE_PATH"
}

remove_package() {
  kysec_call kysec_netctl_pkg_remove us 0 "$SERVICE_NAME"
}

case "${1:-configure}" in
  configure)
    if ! is_kysec_available; then
      exit 0
    fi

    program_registered=0
    package_registered=0
    if register_program; then
      program_registered=1
    else
      log "unable to register $SERVICE_PATH with application network control"
    fi
    if register_package; then
      package_registered=1
    else
      log "unable to register package $SERVICE_NAME with application network control"
    fi

    if [ "$program_registered" -eq 1 ] && [ "$package_registered" -eq 1 ]; then
      log "network-control authorization refreshed for $SERVICE_PATH"
    else
      log "installation continues; approve network access in Kylin Security Center if prompted"
    fi
    ;;
  remove)
    if is_kysec_available; then
      remove_program || log "unable to remove $SERVICE_PATH from application network control"
      remove_package || log "unable to remove package $SERVICE_NAME from application network control"
    fi
    ;;
  *)
    log "usage: $0 [configure|remove]"
    exit 2
    ;;
esac

exit 0
