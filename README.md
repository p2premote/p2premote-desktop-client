# Tauri + Vue + TypeScript

This template should help get you started developing with Vue 3 and TypeScript in Vite. The template uses Vue 3 `<script setup>` SFCs, check out the [script setup docs](https://v3.vuejs.org/api/sfc-script-setup.html#sfc-script-setup) to learn more.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Vue - Official](https://marketplace.visualstudio.com/items?itemName=Vue.volar) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

## Linux release build

Run the unified release build directly from a Linux shell, including a WSL
distribution shell. Docker provides the pinned glibc builder and the same
compilation produces the tar.gz, deb, rpm and Docker image tar artifacts.

```bash
./scripts/build-linux.sh -v 1.6.4
```

On x86_64 WSL Debian, register Docker's QEMU arm64 emulator and pass the
target explicitly to generate the aarch64 packages and image:

```bash
./scripts/build-linux.sh -v 1.6.4 --arch aarch64
```

Run this command inside Linux or WSL; invoking Linux builds from a Windows
PowerShell wrapper is not supported.
