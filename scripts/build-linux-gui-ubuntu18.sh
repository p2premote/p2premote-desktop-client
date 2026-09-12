#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || ! "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 <version>" >&2
  exit 2
fi
version="$1"
repo_dir="$(cd "$(dirname "$0")/.." && pwd)"

command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }
(cd "$repo_dir" && npm ci && npm run build && node scripts/prepare-web-resources.mjs)

for resource in p2premote-service p2premote-cli; do
  [[ -f "$repo_dir/src-tauri/resources/$resource" ]] || {
    echo "missing prebuilt Linux resource: src-tauri/resources/$resource" >&2
    exit 1
  }
done

docker build --platform linux/amd64 \
  -t p2premote-tauri1-ubuntu18 \
  -f "$repo_dir/packaging/linux/gui-ubuntu18/Dockerfile" "$repo_dir"
docker run --rm --platform linux/amd64 \
  -v "$repo_dir:/workspace" -w /workspace \
  -e P2PREMOTE_CLIENT_VERSION="$version" \
  -e P2PREMOTE_PREBUILT_RESOURCES=1 \
  p2premote-tauri1-ubuntu18 \
  cargo tauri build --config 'src-tauri/tauri.linux.conf.json' \
    --config "{\"package\":{\"version\":\"$version\"},\"build\":{\"beforeBuildCommand\":\"\"}}"

mapfile -t elf_files < <(find "$repo_dir/target/release" -maxdepth 1 -type f -perm -111 -print)
[[ ${#elf_files[@]} -gt 0 ]] || { echo "no release ELF artifacts found" >&2; exit 1; }
docker run --rm --platform linux/amd64 -v "$repo_dir:/workspace" -w /workspace \
  p2premote-tauri1-ubuntu18 scripts/check-linux-abi.sh "${elf_files[@]/$repo_dir/\/workspace}"
echo 'Ubuntu 18.04 GUI artifacts are build-ready; compatibility is pending clean-VM validation.'
