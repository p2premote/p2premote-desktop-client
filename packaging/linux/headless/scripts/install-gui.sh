#!/bin/sh
set -u

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
INSTALLER="$SCRIPT_DIR/install-service.sh"
LOG_FILE=$(mktemp "${TMPDIR:-/tmp}/p2premote-install.XXXXXX") || exit 1
trap 'rm -f "$LOG_FILE"' EXIT HUP INT TERM

OS_ID="unknown"
OS_NAME="Linux"
if [ -r /etc/os-release ]; then
  # os-release is defined as shell-compatible variable assignments.
  # shellcheck disable=SC1091
  . /etc/os-release
  OS_ID=${ID:-unknown}
  OS_NAME=${PRETTY_NAME:-${NAME:-Linux}}
fi

case "$OS_ID" in
  deepin|uos) DESKTOP_FAMILY="DDE" ;;
  kylin|ubuntukylin|openkylin) DESKTOP_FAMILY="UKUI" ;;
  *) DESKTOP_FAMILY=${XDG_CURRENT_DESKTOP:-unknown} ;;
esac

show_message() {
  level="$1"
  title="$2"
  message="$3"

  if command -v zenity >/dev/null 2>&1; then
    zenity "--$level" --title="$title" --text="$message" --width=430
  elif command -v yad >/dev/null 2>&1; then
    yad "--$level" --title="$title" --text="$message" --width=430 --button=确定:0
  elif command -v kdialog >/dev/null 2>&1; then
    if [ "$level" = "error" ]; then
      kdialog --error "$message" --title "$title"
    else
      kdialog --msgbox "$message" --title "$title"
    fi
  elif command -v xmessage >/dev/null 2>&1; then
    xmessage -center -title "$title" "$message"
  else
    printf '%s\n%s\n' "$title" "$message"
  fi
}

if [ ! -f "$INSTALLER" ]; then
  show_message error "p2pRemote 安装失败" "找不到安装程序：\n$INSTALLER\n\n请重新解压完整的安装包后再试。"
  exit 1
fi

if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ]; then
  printf '%s\n' "未检测到图形桌面。请在终端中运行：sudo ./install-service.sh" >&2
  exit 1
fi

if ! command -v pkexec >/dev/null 2>&1; then
  show_message error "p2pRemote 安装失败" "$OS_NAME 缺少图形化授权组件 pkexec。\n\nDeepin/UOS 请安装 polkit 和 DDE 授权代理；\n麒麟/openKylin 请安装 policykit-1 和 UKUI 授权代理。\n\n也可以在终端中运行：\nsudo ./install-service.sh"
  exit 1
fi

# pkexec displays the desktop's native administrator-password dialog. Keep all
# privileged work in the existing installer so this launcher stays unprivileged.
pkexec "$INSTALLER" >"$LOG_FILE" 2>&1
status=$?
if [ "$status" -eq 0 ]; then
  show_message info "p2pRemote 安装完成" "p2pRemote 已安装并启动。\n\n管理页面：\nhttp://127.0.0.1:48083/\n\n首次使用请在页面引导下设置 Web 安全码。"
  if command -v xdg-open >/dev/null 2>&1; then
    xdg-open "http://127.0.0.1:48083/" >/dev/null 2>&1 &
  fi
  exit 0
fi

details=$(tail -n 12 "$LOG_FILE" 2>/dev/null)
if [ -z "$details" ]; then
  details="授权已取消，或安装程序未能启动。"
fi
if [ "$status" -eq 126 ] || [ "$status" -eq 127 ]; then
  details="管理员授权未完成。请确认 $DESKTOP_FAMILY 的 Polkit 授权代理正在运行。\n\n$details"
fi
show_message error "p2pRemote 安装失败" "系统：$OS_NAME\n桌面：$DESKTOP_FAMILY\n\n安装没有完成。\n\n$details"
exit "$status"
