#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || ! "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 <version>" >&2
  exit 2
fi
version="$1"
repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
source_root="$(cd "$repo_dir/.." && pwd)"
project_name="$(basename "$repo_dir")"
frontend_dir="$(mktemp -d)"
generated_config="$repo_dir/src-tauri/tauri.ubuntu18.generated.conf.json"
cleanup() {
  rm -rf "$frontend_dir"
  rm -f "$generated_config"
}
trap cleanup EXIT

command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }
command -v node >/dev/null || { echo "node is required" >&2; exit 1; }
command -v npm >/dev/null || { echo "npm is required" >&2; exit 1; }

# Keep Linux npm artifacts away from a Windows node_modules tree on WSL/DrvFS.
cp "$repo_dir"/{package.json,package-lock.json,index.html,tsconfig.json,tsconfig.node.json,vite.config.ts} "$frontend_dir/"
cp -a "$repo_dir/src" "$repo_dir/public" "$frontend_dir/"
(cd "$frontend_dir" && npm ci && npm run build)
rm -rf "$repo_dir/dist"
cp -a "$frontend_dir/dist" "$repo_dir/dist"
(cd "$repo_dir" && node scripts/prepare-web-resources.mjs)

node - "$repo_dir/src-tauri/tauri.linux.conf.json" "$generated_config" "$version" <<'NODE'
import fs from 'node:fs'
const [, , source, destination, version] = process.argv
const config = JSON.parse(fs.readFileSync(source, 'utf8'))
config.package = { ...(config.package ?? {}), version }
fs.writeFileSync(destination, `${JSON.stringify(config, null, 2)}\n`)
NODE

docker_build_args=()
if [[ -n "${P2PREMOTE_BUILD_PROXY:-}" ]]; then
  docker_build_args+=(
    --build-arg "HTTP_PROXY=$P2PREMOTE_BUILD_PROXY"
    --build-arg "HTTPS_PROXY=$P2PREMOTE_BUILD_PROXY"
  )
fi

builder_image="${P2PREMOTE_GUI_BUILDER_IMAGE:-p2premote-tauri1-ubuntu18-rust1.77:v1}"
if ! docker image inspect "$builder_image" >/dev/null 2>&1; then
  echo "==> GUI builder image $builder_image not found; building it once"
  docker build --platform linux/amd64 \
    "${docker_build_args[@]}" \
    -t "$builder_image" \
    -f "$repo_dir/packaging/linux/gui-ubuntu18/Dockerfile" \
    "$repo_dir/packaging/linux/gui-ubuntu18"
else
  echo "==> Reusing GUI builder image $builder_image"
fi

docker_env=(
  -e P2PREMOTE_CLIENT_VERSION="$version"
  -e P2PREMOTE_PREBUILT_RESOURCES=1
  -e CARGO_NET_GIT_FETCH_WITH_CLI=true
  -e TAURI_TRAY=appindicator
  -e CARGO_TARGET_DIR="/workspace/$project_name/target/linux-gui-x64"
)
if [[ -n "${P2PREMOTE_BUILD_PROXY:-}" ]]; then
  for name in HTTP_PROXY HTTPS_PROXY ALL_PROXY http_proxy https_proxy all_proxy; do
    docker_env+=( -e "$name=$P2PREMOTE_BUILD_PROXY" )
  done
fi

docker run --rm --platform linux/amd64 \
  -v "$source_root:/workspace" \
  -v p2premote-cargo-ubuntu18:/root/.cargo/registry \
  -v p2premote-cargo-git-ubuntu18:/root/.cargo/git \
  -v p2premote-go-ubuntu18:/root/go/pkg/mod \
  -v p2premote-tauri-cache-ubuntu18:/root/.cache/tauri \
  -w "/workspace/$project_name" \
  "${docker_env[@]}" \
  "$builder_image" \
  bash scripts/build-linux-gui-ubuntu18-container.sh \
    'src-tauri/tauri.ubuntu18.generated.conf.json'

"$repo_dir/scripts/validate-linux-gui-ubuntu18.sh" "$version"
