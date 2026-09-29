#!/usr/bin/env bash
set -euo pipefail

# 确保 src-tauri/resources/ 下存在指定的 RustDeskTiny 发布产物。
# 本地已存在时直接复用（本地构建语义，不校验哈希）；缺失时按
# scripts/rustdesktiny-artifacts.env 钉版从 GitHub Release 下载并校验 SHA-256。
#
# 用法: fetch-rustdesktiny-artifact.sh <windows-x64|windows-win7-x64|linux-amd64|macos-x86_64|macos-aarch64>

usage() {
  echo "usage: $0 <windows-x64|windows-win7-x64|linux-amd64|macos-x86_64|macos-aarch64>" >&2
  exit 2
}

[[ $# -eq 1 ]] || usage
kind="$1"

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
resources_dir="$repo_dir/src-tauri/resources"
pin_file="$repo_dir/scripts/rustdesktiny-artifacts.env"
[[ -f "$pin_file" ]] || { echo "missing RustDeskTiny pin file: $pin_file" >&2; exit 1; }
# shellcheck source=/dev/null
source "$pin_file"

case "$kind" in
  windows-x64)
    asset="RustDeskTiny-install.exe"
    release_asset="$asset"
    base_url="$RUSTDESK_TINY_BASE_URL"
    version="$RUSTDESK_TINY_VERSION"
    sha="$RUSTDESK_TINY_WINDOWS_X64_SHA256" ;;
  windows-win7-x64)
    # RustDeskTinyLegacy 是独立仓库，版本线与主仓分开
    asset="RustDeskTinyLegacy-install.exe"
    release_asset="$asset"
    base_url="$RUSTDESK_TINY_LEGACY_BASE_URL"
    version="$RUSTDESK_TINY_LEGACY_VERSION"
    sha="$RUSTDESK_TINY_WINDOWS_WIN7_X64_SHA256" ;;
  linux-amd64)
    # 本地沿用 repack-linux-gui-deb.sh 的约定名 RustDeskTiny.deb；
    # Release 上的资产名带版本号
    asset="RustDeskTiny.deb"
    release_asset="RustDeskTiny-${RUSTDESK_TINY_VERSION}.deb"
    base_url="$RUSTDESK_TINY_BASE_URL"
    version="$RUSTDESK_TINY_VERSION"
    sha="$RUSTDESK_TINY_LINUX_AMD64_SHA256" ;;
  macos-x86_64)
    asset="RustDeskTiny-macos-x86_64.zip"
    release_asset="$asset"
    base_url="$RUSTDESK_TINY_BASE_URL"
    version="${RUSTDESK_TINY_MACOS_VERSION:-$RUSTDESK_TINY_VERSION}"
    sha="$RUSTDESK_TINY_MACOS_X86_64_SHA256" ;;
  macos-aarch64)
    asset="RustDeskTiny-macos-aarch64.zip"
    release_asset="$asset"
    base_url="$RUSTDESK_TINY_BASE_URL"
    version="${RUSTDESK_TINY_MACOS_VERSION:-$RUSTDESK_TINY_VERSION}"
    sha="$RUSTDESK_TINY_MACOS_AARCH64_SHA256" ;;
  *) usage ;;
esac

target="$resources_dir/$asset"
if [[ -f "$target" ]]; then
  echo "Reusing local RustDeskTiny artifact: $target"
  exit 0
fi

# 只有确实需要下载时才要求钉版信息齐全（本地复用不校验）
[[ -n "${base_url:-}" && -n "${version:-}" && -n "$sha" ]] || {
  echo "RustDeskTiny pin file is missing base URL, version, or the $kind SHA-256" >&2
  exit 1
}

command -v curl >/dev/null || { echo "curl is required to download $asset" >&2; exit 1; }

compute_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

url="$base_url/$version/$release_asset"
partial="$target.partial"
trap 'rm -f "$partial"' EXIT
echo "Downloading $url"
mkdir -p "$resources_dir"
curl_args=(-fL --retry 3 --connect-timeout 15 -o "$partial")
# 标准代理环境变量 curl 自行识别；仓库约定的 P2PREMOTE_BUILD_PROXY 显式传入
[[ -z "${P2PREMOTE_BUILD_PROXY:-}" ]] || curl_args+=(-x "$P2PREMOTE_BUILD_PROXY")
curl "${curl_args[@]}" "$url" || { echo "download failed: $url" >&2; exit 1; }

actual_sha="$(compute_sha256 "$partial")"
[[ "$actual_sha" == "$sha" ]] || {
  echo "SHA-256 mismatch for $asset: expected $sha, got $actual_sha" >&2
  exit 1
}
mv "$partial" "$target"
echo "Verified RustDeskTiny $kind artifact: $target"
