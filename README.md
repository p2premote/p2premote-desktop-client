# Tauri + Vue + TypeScript

This template should help get you started developing with Vue 3 and TypeScript in Vite. The template uses Vue 3 `<script setup>` SFCs, check out the [script setup docs](https://v3.vuejs.org/api/sfc-script-setup.html#sfc-script-setup) to learn more.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Vue - Official](https://marketplace.visualstudio.com/items?itemName=Vue.volar) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

## Linux headless build

Run the build directly from a Linux shell, including a WSL distribution shell.
The build uses the host's GNU/glibc toolchain and produces a package for that
host architecture.

```bash
./scripts/build-linux-headless.sh -v 1.6.4
```

Run this command inside Linux or WSL; invoking Linux builds from a Windows
PowerShell wrapper is not supported.
