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
punch_dir="${P2PREMOTE_PUNCH_DIR:-$(cd "$repo_dir/../p2premote-punch" && pwd)}"
resources_dir="$repo_dir/src-tauri/resources"
tiny_dir="${RUSTDESK_TINY_DIR:-$(cd "$repo_dir/../remoteDesk/RustDeskTiny" && pwd)}"
tiny_build_script="$tiny_dir/scripts/build-macos-tiny.sh"
tiny_artifact="$tiny_dir/dist/macos-universal-release/RustDeskTiny.app"
build_dir="$(mktemp -d "${TMPDIR:-/tmp}/p2premote-macos.XXXXXX")"
trap 'rm -rf "$build_dir"' EXIT

for command in cargo rustup go npm npx lipo install_name_tool codesign xcrun hdiutil; do
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

mkdir -p "$resources_dir"
[[ -f "$tiny_build_script" ]] || { echo "RustDeskTiny macOS build script not found: $tiny_build_script" >&2; exit 1; }
bash "$tiny_build_script"
[[ -d "$tiny_artifact" ]] || { echo "RustDeskTiny universal app is missing after build: $tiny_artifact" >&2; exit 1; }
rm -rf "$resources_dir/RustDeskTiny.app"
cp -a "$tiny_artifact" "$resources_dir/RustDeskTiny.app"
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

build_punch() {
  local goarch="$1"
  local clang_arch="$2"
  local output="$3"
  (
    cd "$punch_dir"
    CGO_ENABLED=1 GOOS=darwin GOARCH="$goarch" CC="clang -arch $clang_arch" \
      go build -buildmode=c-shared -ldflags "-s -w" -o "$output" ./punchffi
  )
  install_name_tool -id "@rpath/libp2premote-punch.dylib" "$output"
  rm -f "${output%.dylib}.h"
}

build_punch amd64 x86_64 "$build_dir/x86_64/libp2premote-punch.dylib"
build_punch arm64 arm64 "$build_dir/arm64/libp2premote-punch.dylib"
build_rust_helper x86_64-apple-darwin "$build_dir/x86_64"
build_rust_helper aarch64-apple-darwin "$build_dir/arm64"

for artifact in p2premote-service p2premote-cli libp2premote-punch.dylib; do
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
    --sign "$APPLE_SIGNING_IDENTITY" "$resources_dir/libp2premote-punch.dylib"
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
      P2PREMOTE_PUNCH_DIR="$punch_dir" \
      P2PREMOTE_PREBUILT_RESOURCES=1 \
      npx tauri build --target universal-apple-darwin --bundles app --config "$macos_config"
  else
    P2PREMOTE_CLIENT_VERSION="$version" \
    P2PREMOTE_PUNCH_DIR="$punch_dir" \
    P2PREMOTE_PREBUILT_RESOURCES=1 \
    APPLE_SIGNING_IDENTITY="$APPLE_SIGNING_IDENTITY" \
      npx tauri build --target universal-apple-darwin --bundles app --config "$macos_config"
  fi
)

app_path="$(find "$cargo_target_dir/universal-apple-darwin/release/bundle/macos" -maxdepth 1 -name '*.app' -print -quit)"
[[ -n "$app_path" ]] || { echo "Universal app bundle was not produced." >&2; exit 1; }
mkdir -p "$app_path/Contents/Library/LaunchDaemons"
cp "$repo_dir/src-tauri/macos/top.p2premote.service.plist" \
  "$app_path/Contents/Library/LaunchDaemons/top.p2premote.service.plist"
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
for artifact in p2premote-service p2premote-cli libp2premote-punch.dylib; do
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
