#!/usr/bin/env bash
set -euo pipefail

export LANG=${LANG:-en_US.UTF-8}
export LC_ALL=${LC_ALL:-en_US.UTF-8}

usage() {
  echo "Usage: scripts/build-macos.sh -v <version> [--no-sccache] [--unsigned]"
}

version=""
use_sccache=1
unsigned=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    -v)
      [[ $# -ge 2 && -n "$2" ]] || { usage >&2; exit 2; }
      version="$2"
      shift 2
      ;;
    --no-sccache) use_sccache=0; shift ;;
    --unsigned) unsigned=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS packages must be built on a macOS host." >&2
  exit 1
fi
if [[ -z "$version" ]]; then
  usage >&2
  exit 2
fi
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]]; then
  echo "version must be valid SemVer, for example 1.11.2 or 1.11.2-ebff2c" >&2
  exit 2
fi

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
cargo_target_dir="$repo_dir/target/macos-universal"
dist_dir="$repo_dir/artifacts/macos-universal"
export CARGO_TARGET_DIR="$cargo_target_dir"
# Punch/Exchange 与 Windows/Linux 一致由 p2premote-punch-rs 源码集成；
# Go 侧只构建 p2premote-wg-ffi（wgonly）的 userspace WireGuard 数据面 dylib，
# 与 Windows 的 p2premote-wg.dll 同源同参数（GOTOOLCHAIN 钉版仅 Windows 的
# Win7 兼容需要，macOS 用本机 Go >= 1.20 即可）。
wg_ffi_dir="${P2PREMOTE_WG_FFI_DIR:-$(cd "$repo_dir/../p2premote-wg-ffi" && pwd)}"
resources_dir="$repo_dir/src-tauri/resources"
# RustDeskTiny 使用预编译产物（与 Windows 的 RustDeskTiny-install.exe、Linux 的
# RustDeskTiny.deb 同一模式）：两个单架构 zip 来自 RustDeskTiny GitHub Release
# （p2premote/rustdeskTiny，CI 用 ditto 归档），本地缺失时按
# scripts/rustdesktiny-artifacts.env 钉版自动下载；脚本解包后 lipo 合成 universal
# app，不再依赖源码全量编译（无需 remoteDesk 检出、Xcode/Flutter/vcpkg 工具链）。
# RUSTDESK_TINY_APP 可直接指向已合成的 universal RustDeskTiny.app 复用。
tiny_app="${RUSTDESK_TINY_APP:-}"
tiny_zip_x64="$resources_dir/RustDeskTiny-macos-x86_64.zip"
tiny_zip_arm64="$resources_dir/RustDeskTiny-macos-aarch64.zip"
build_dir="$(mktemp -d "${TMPDIR:-/tmp}/p2premote-macos.XXXXXX")"
trap 'rm -rf "$build_dir"' EXIT

for command in cargo rustup go npm npx lipo install_name_tool codesign xcrun hdiutil ditto file; do
  command -v "$command" >/dev/null || { echo "Missing required command: $command" >&2; exit 1; }
done
if [[ "$use_sccache" == 1 ]]; then
  command -v sccache >/dev/null 2>&1 || { echo "sccache is required by default; install it or pass --no-sccache" >&2; exit 1; }
  export RUSTC_WRAPPER=sccache
  echo "==> sccache enabled (use --no-sccache to disable)"
else
  export RUSTC_WRAPPER=""
  echo "==> sccache disabled; Rust will compile locally"
fi

is_macho() {
  file -b "$1" | grep -q '^Mach-O'
}

# 以 x86_64 包为基准复制后，逐个把 arm64 包中的 Mach-O 切片 lipo 进去。
# 基准选 x86_64 是因为 Info.plist 等非二进制资源两个架构相同，而 x86_64 侧的
# LSMinimumSystemVersion 更低，老 Intel Mac 才能继续安装合并后的包。
merge_universal_app() {
  local x64_app="$1"
  local arm_app="$2"
  local out_app="$3"
  rm -rf "$out_app"
  mkdir -p "$(dirname "$out_app")"
  cp -a "$x64_app" "$out_app"

  local arm_file rel out_file merged=0
  # find 必须输出绝对路径：rel 的前缀剥除依赖 $arm_app 开头，且 file/lipo
  # 会在脚本 cwd 而非 arm_app 下解析相对路径（曾经导致 arm64 侧全部
  # is_macho 失败、无一切片被合并）。
  while IFS= read -r -d '' arm_file; do
    rel="${arm_file#"$arm_app"/}"
    out_file="$out_app/$rel"
    if [[ ! -f "$out_file" ]]; then
      echo "Warning: file exists only in the arm64 bundle, skipping: $rel" >&2
      continue
    fi
    if [[ "$rel" == *.plist ]] && ! cmp -s "$out_file" "$arm_file"; then
      echo "Note: $rel differs between arch bundles; keeping the x86_64 copy"
    fi
    if is_macho "$out_file" && is_macho "$arm_file"; then
      if ! lipo -create "$out_file" "$arm_file" -output "$out_file"; then
        echo "Error: lipo -create failed for $rel; both sides:" >&2
        file "$out_file" "$arm_file" >&2
        exit 1
      fi
      merged=$((merged + 1))
    fi
  done < <(find "$arm_app" -type f -print0)

  for binary in RustDeskTiny service; do
    if ! lipo "$out_app/Contents/MacOS/$binary" -verify_arch x86_64 arm64; then
      echo "Error: $binary lacks x86_64/arm64 slice after merge:" >&2
      lipo -info "$out_app/Contents/MacOS/$binary" >&2 || true
      exit 1
    fi
  done
  echo "==> Merged $merged Mach-O slices into the universal RustDeskTiny.app"
}

mkdir -p "$resources_dir"
if [[ -z "$tiny_app" ]]; then
  # 产物不进 git：本地已有直接复用，缺失时（CI 等干净环境）按钉版自动下载并校验
  "$repo_dir/scripts/fetch-rustdesktiny-artifact.sh" macos-x86_64
  "$repo_dir/scripts/fetch-rustdesktiny-artifact.sh" macos-aarch64
  echo "==> Merging RustDeskTiny prebuilt archives into a universal app"
  ditto -x -k "$tiny_zip_x64" "$build_dir/tiny-x86_64"
  ditto -x -k "$tiny_zip_arm64" "$build_dir/tiny-arm64"
  merge_universal_app \
    "$build_dir/tiny-x86_64/RustDeskTiny.app" \
    "$build_dir/tiny-arm64/RustDeskTiny.app" \
    "$build_dir/tiny-universal/RustDeskTiny.app"
  tiny_app="$build_dir/tiny-universal/RustDeskTiny.app"
fi
[[ -d "$tiny_app" ]] || { echo "RustDeskTiny.app is missing: $tiny_app" >&2; exit 1; }
[[ -f "$tiny_app/Contents/MacOS/RustDeskTiny" && -f "$tiny_app/Contents/MacOS/service" ]] || {
  echo "RustDeskTiny.app is incomplete (needs Contents/MacOS/RustDeskTiny and service): $tiny_app" >&2
  exit 1
}
rm -rf "$resources_dir/RustDeskTiny.app"
cp -a "$tiny_app" "$resources_dir/RustDeskTiny.app"
rustup target add x86_64-apple-darwin aarch64-apple-darwin
mkdir -p "$resources_dir" "$build_dir/x86_64" "$build_dir/arm64"

build_rust_helper() {
  local triple="$1"
  local arch_dir="$2"
  P2PREMOTE_CLIENT_VERSION="$version" \
  P2PREMOTE_PUNCH_LIB_DIR="$arch_dir" cargo build \
    --manifest-path "$repo_dir/Cargo.toml" \
    --release --target "$triple" \
    -p p2premote-service -p p2premote-cli
  cp "$cargo_target_dir/$triple/release/p2premote-service" "$arch_dir/p2premote-service"
  cp "$cargo_target_dir/$triple/release/p2premote-cli" "$arch_dir/p2premote-cli"
}

build_wg() {
  local goarch="$1"
  local clang_arch="$2"
  local output="$3"
  (
    cd "$wg_ffi_dir"
    CGO_ENABLED=1 GOOS=darwin GOARCH="$goarch" CC="clang -arch $clang_arch" \
      go build -tags wgonly -mod=mod \
      -buildmode=c-shared -ldflags "-s -w" -o "$output" ./punchffi
  )
  install_name_tool -id "@rpath/libp2premote-wg.dylib" "$output"
  rm -f "${output%.dylib}.h"
}

build_wg amd64 x86_64 "$build_dir/x86_64/libp2premote-wg.dylib"
build_wg arm64 arm64 "$build_dir/arm64/libp2premote-wg.dylib"
build_rust_helper x86_64-apple-darwin "$build_dir/x86_64"
build_rust_helper aarch64-apple-darwin "$build_dir/arm64"

for artifact in p2premote-service p2premote-cli libp2premote-wg.dylib; do
  lipo -create \
    "$build_dir/x86_64/$artifact" \
    "$build_dir/arm64/$artifact" \
    -output "$resources_dir/$artifact"
  lipo "$resources_dir/$artifact" -verify_arch x86_64 arm64
done

if [[ "$unsigned" == 0 && -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  codesign --force --deep --timestamp --options runtime \
    --sign "$APPLE_SIGNING_IDENTITY" "$resources_dir/RustDeskTiny.app"
  codesign --force --timestamp --options runtime \
    --sign "$APPLE_SIGNING_IDENTITY" "$resources_dir/libp2premote-wg.dylib"
  codesign --force --timestamp --options runtime \
    --sign "$APPLE_SIGNING_IDENTITY" "$resources_dir/p2premote-service"
  codesign --force --timestamp --options runtime \
    --sign "$APPLE_SIGNING_IDENTITY" "$resources_dir/p2premote-cli"
elif [[ "$unsigned" == 0 ]]; then
  echo "APPLE_SIGNING_IDENTITY is required for a distributable Developer ID build." >&2
  exit 1
fi

(cd "$repo_dir" && npm ci && npm run build)
macos_config="$build_dir/tauri.macos.conf.json"
{
  printf '{\n  "package": { "version": "%s" },\n' "$version"
  tail -n +2 "$repo_dir/src-tauri/tauri.macos.conf.json"
} > "$macos_config"
(
  cd "$repo_dir"
  if [[ "$unsigned" == 1 ]]; then
    env -u APPLE_SIGNING_IDENTITY \
      P2PREMOTE_CLIENT_VERSION="$version" \
      P2PREMOTE_PREBUILT_RESOURCES=1 \
      npx tauri build --target universal-apple-darwin --bundles app --config "$macos_config"
  else
    P2PREMOTE_CLIENT_VERSION="$version" \
    P2PREMOTE_PREBUILT_RESOURCES=1 \
    APPLE_SIGNING_IDENTITY="$APPLE_SIGNING_IDENTITY" \
      npx tauri build --target universal-apple-darwin --bundles app --config "$macos_config"
  fi
)

app_path="$(find "$cargo_target_dir/universal-apple-darwin/release/bundle/macos" -maxdepth 1 -name '*.app' -print -quit)"
[[ -n "$app_path" ]] || { echo "Universal app bundle was not produced." >&2; exit 1; }
if [[ "$unsigned" == 1 ]]; then
  codesign --force --deep \
    --entitlements "$repo_dir/src-tauri/Entitlements.plist" \
    --sign - "$app_path"
else
  codesign --force --deep --timestamp --options runtime \
    --entitlements "$repo_dir/src-tauri/Entitlements.plist" \
    --sign "$APPLE_SIGNING_IDENTITY" "$app_path"
fi
lipo "$app_path/Contents/MacOS/p2pRemote" -verify_arch x86_64 arm64
for artifact in p2premote-service p2premote-cli libp2premote-wg.dylib; do
  lipo "$app_path/Contents/Resources/resources/$artifact" -verify_arch x86_64 arm64
done
lipo "$app_path/Contents/Resources/resources/RustDeskTiny.app/Contents/MacOS/RustDeskTiny" \
  -verify_arch x86_64 arm64
codesign --verify --deep --strict --verbose=2 "$app_path"

mkdir -p "$dist_dir"
dmg_path="$dist_dir/p2pRemote_${version}_macos-universal.dmg"
rm -f "$dmg_path"
hdiutil create -volname p2pRemote -srcfolder "$app_path" -ov -format UDZO "$dmg_path"

if [[ "$unsigned" == 1 ]]; then
  echo "Unsigned test build requested; skipping Apple notarization and Gatekeeper assessment."
elif [[ -n "${APPLE_NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
  xcrun notarytool submit "$dmg_path" \
    --keychain-profile "$APPLE_NOTARY_KEYCHAIN_PROFILE" --wait
else
  : "${APPLE_ID:?Set APPLE_NOTARY_KEYCHAIN_PROFILE or APPLE_ID}"
  : "${APPLE_TEAM_ID:?Set APPLE_TEAM_ID for notarization}"
  : "${APPLE_APP_PASSWORD:?Set APPLE_APP_PASSWORD for notarization}"
  xcrun notarytool submit "$dmg_path" \
    --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" --wait
fi

if [[ "$unsigned" == 0 ]]; then
  xcrun stapler staple "$dmg_path"
  xcrun stapler validate "$dmg_path"
  if [[ -n "$app_path" ]]; then
    xcrun stapler staple "$app_path"
    xcrun stapler validate "$app_path"
    spctl --assess --type execute -v "$app_path"
  fi
  spctl --assess --type open --context context:primary-signature -v "$dmg_path"
fi

echo "Created $dmg_path"

# ---- pkg 安装包（正式分发格式：app + LaunchDaemon，postinstall 以 root 完成
# machine 目录/服务安装并启动；DMG 仅保留作内部测试） ----
# 执行位兜底：Windows 检出的工作树可能丢失 postinstall 的 +x，pkgbuild 原样
# 打包会让安装器无法执行脚本（PKInstallErrorDomain 112）。
chmod +x "$repo_dir/scripts/macos-pkg/postinstall"
pkg_staging="$build_dir/pkg-root"
rm -rf "$pkg_staging"
mkdir -p "$pkg_staging/Applications" "$pkg_staging/Library/LaunchDaemons"
cp -R "$app_path" "$pkg_staging/Applications/$(basename "$app_path")"
cp "$repo_dir/src-tauri/macos/top.p2premote.service.plist" \
  "$pkg_staging/Library/LaunchDaemons/top.p2premote.service.plist"
pkg_path="$dist_dir/p2pRemote_${version}_macos-universal.pkg"
rm -f "$pkg_path"
pkgbuild \
  --root "$pkg_staging" \
  --identifier top.p2premote.client \
  --version "$version" \
  --scripts "$repo_dir/scripts/macos-pkg" \
  "$pkg_path"

echo "Created $pkg_path"
