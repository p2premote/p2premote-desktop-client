#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: scripts/build-macos.sh -v <version>"
}

version=""
while getopts ":v:h" option; do
  case "$option" in
    v) version="$OPTARG" ;;
    h) usage; exit 0 ;;
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

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
punch_dir="${P2PREMOTE_PUNCH_DIR:-$(cd "$repo_dir/../p2premote-punch" && pwd)}"
resources_dir="$repo_dir/src-tauri/resources"
engine_dir="$(cd "$repo_dir/../remoteDesk/remote-desktop-engine" && pwd)"
engine_build_script="$engine_dir/scripts/build-macos.sh"
engine_artifact="$engine_dir/artifacts/macos-universal/p2premote-desktop-engine"
build_dir="$(mktemp -d "${TMPDIR:-/tmp}/p2premote-macos.XXXXXX")"
trap 'rm -rf "$build_dir"' EXIT

for command in cargo rustup go npm npx lipo install_name_tool codesign xcrun; do
  command -v "$command" >/dev/null || { echo "Missing required command: $command" >&2; exit 1; }
done

mkdir -p "$resources_dir"
[[ -x "$engine_build_script" ]] || { echo "macOS Engine build script not found or not executable: $engine_build_script" >&2; exit 1; }
"$engine_build_script"
[[ -f "$engine_artifact" ]] || { echo "macOS Engine artifact is missing after build: $engine_artifact" >&2; exit 1; }
cp "$engine_artifact" "$resources_dir/p2premote-desktop-engine"
chmod 755 "$resources_dir/p2premote-desktop-engine"

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
  cp "$repo_dir/target/$triple/release/p2premote-service" "$arch_dir/p2premote-service"
  cp "$repo_dir/target/$triple/release/p2premote-cli" "$arch_dir/p2premote-cli"
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

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  codesign --force --timestamp --options runtime \
    --sign "$APPLE_SIGNING_IDENTITY" "$resources_dir/libp2premote-punch.dylib"
  codesign --force --timestamp --options runtime \
    --sign "$APPLE_SIGNING_IDENTITY" "$resources_dir/p2premote-service"
  codesign --force --timestamp --options runtime \
    --sign "$APPLE_SIGNING_IDENTITY" "$resources_dir/p2premote-cli"
  codesign --force --timestamp --options runtime \
    --sign "$APPLE_SIGNING_IDENTITY" "$resources_dir/p2premote-desktop-engine"
else
  echo "APPLE_SIGNING_IDENTITY is required for a distributable Developer ID build." >&2
  exit 1
fi

(cd "$repo_dir" && npm ci && npm run build)
(
  cd "$repo_dir"
  P2PREMOTE_CLIENT_VERSION="$version" \
  P2PREMOTE_PUNCH_DIR="$punch_dir" \
  P2PREMOTE_PREBUILT_RESOURCES=1 \
  APPLE_SIGNING_IDENTITY="$APPLE_SIGNING_IDENTITY" \
    npx tauri build --target universal-apple-darwin --config "{\"version\":\"$version\"}"
)

dmg_path="$(find "$repo_dir/target/universal-apple-darwin/release/bundle/dmg" -maxdepth 1 -name '*.dmg' -print -quit)"
if [[ -z "$dmg_path" ]]; then
  echo "Universal DMG was not produced." >&2
  exit 1
fi

app_path="$(find "$repo_dir/target/universal-apple-darwin/release/bundle/macos" -maxdepth 1 -name '*.app' -print -quit)"
if [[ -n "$app_path" ]]; then
  lipo "$app_path/Contents/MacOS/p2pRemote" -verify_arch x86_64 arm64
  for artifact in p2premote-service p2premote-cli p2premote-desktop-engine libp2premote-punch.dylib; do
    lipo "$app_path/Contents/Resources/resources/$artifact" -verify_arch x86_64 arm64
  done
  test -f "$app_path/Contents/Library/LaunchDaemons/top.p2premote.service.plist"
  codesign --verify --deep --strict --verbose=2 "$app_path"
fi

if [[ -n "${APPLE_NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
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

xcrun stapler staple "$dmg_path"
xcrun stapler validate "$dmg_path"
if [[ -n "$app_path" ]]; then
  xcrun stapler staple "$app_path"
  xcrun stapler validate "$app_path"
  spctl --assess --type execute -v "$app_path"
fi
spctl --assess --type open --context context:primary-signature -v "$dmg_path"

release_name="p2pRemote_${version}_universal.dmg"
cp "$dmg_path" "$repo_dir/target/$release_name"
echo "Created $repo_dir/target/$release_name"
