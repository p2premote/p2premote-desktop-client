# 桌面客户端编译说明

本文档覆盖桌面客户端的 5 种交付产物。所有版本号都使用 `-v` 传入，格式为三段式 SemVer，例如 `1.6.4`。脚本会把当前 Git 短提交号追加到内部构建版本中。

## 产物总览

| 产物 | 架构 | 编译入口 |
| --- | --- | --- |
| Windows GUI 安装包 | 当前 Windows 主机架构（通常为 x64） | `scripts/build-windows.ps1` |
| Linux Headless 压缩包 | x86_64 | `scripts/build-linux-headless.sh` |
| Linux Headless 压缩包 | aarch64 | `scripts/build-linux-headless.sh` |
| Linux Headless Docker 镜像 | x86_64 | `scripts/build-docker.sh` |
| Linux Headless Docker 镜像 | aarch64 | `scripts/build-docker.sh` |

## 1. Windows GUI

在 PowerShell 中执行：

```powershell
.\scripts\build-windows.ps1 -v 1.6.4
```

脚本会更新 Windows 客户端版本信息，然后执行 `npx tauri build`。NSIS 安装包通常位于：

```text
src-tauri\target\release\bundle\nsis\
```

要求已安装 Node.js、Rust、Tauri CLI、Windows 编译工具链，并且相邻目录 `../p2premote-punch` 必须存在；脚本会把当前版本和该源码目录显式传给 Tauri 构建。任何必需工具、源码或资源缺失都会直接失败。

## 2. Linux Headless x86_64

必须在 x86_64 Linux 或 x86_64 WSL shell 中执行，不能从 Windows PowerShell 直接调用 Linux 脚本：

```bash
./scripts/build-linux-headless.sh -v 1.6.4
```

需要 Node.js 22 或更高版本、npm、Rust、Go、C/C++ 编译工具链和与主机架构匹配的 `x86_64-linux-musl-gcc` 或 `aarch64-linux-musl-gcc`。产物位于：

```text
build/linux/dist/headless/p2premote-headless_<version>-<git-sha>_x86_64-linux-gnu.tar.gz
```

## 3. Linux Headless aarch64

必须在 aarch64 Linux 主机或 aarch64 Linux 环境中执行同一个脚本：

```bash
./scripts/build-linux-headless.sh -v 1.6.4
```

产物位于：

```text
build/linux/dist/headless/p2premote-headless_<version>-<git-sha>_aarch64-linux-gnu.tar.gz
```

当前脚本根据 `uname -m` 选择架构；它不会在 x86_64 主机上自动把完整 Headless 客户端交叉编译成 aarch64。`packaging/linux/builder/` 中的 Ubuntu 16/glibc 2.23 镜像可用于准备对应架构的编译环境，但完整打包仍应在对应架构环境中执行或先接入专用 Docker 构建流程。

Linux 构建脚本要求架构和编译器严格匹配；缺少目标架构工具链、产物不存在或产物架构不匹配时会直接失败，不会回退到宿主架构或其他 tar 包。

在 Ubuntu 16/glibc 2.23 builder 中，官方 Node 22 二进制的 glibc 要求高于基线，因此前端需要先在 Node.js 22 环境构建一次，再在 builder 中复用 `dist` 完成 ARM 原生构建：

```bash
# 在 Node.js 22 环境中执行
npm ci
npm run build

# 切换到 Ubuntu 16/glibc 2.23 aarch64 builder 后执行
P2PREMOTE_SKIP_WEB_BUILD=1 ./scripts/build-linux-headless.sh -v 1.6.4
```

`P2PREMOTE_SKIP_WEB_BUILD=1` 只跳过前端编译，仍会重新编译 punch、wireguard、Rust service/CLI，并把已有的 `dist` 打入包中。

## 4. Linux Headless Docker x86_64

在 x86_64 Linux 或 x86_64 WSL shell 中执行：

```bash
./scripts/build-docker.sh -v 1.6.4
```

该脚本会先调用 Linux Headless 编译，再构建并保存 Docker 镜像。默认镜像标签为：

```text
p2premote/client:1.6.4
```

镜像归档位于：

```text
build/linux/dist/docker/p2premote-client_1.6.4.tar
```

也可以跳过 Headless 编译，直接指定已有压缩包：

```bash
./scripts/build-docker.sh -v 1.6.4 \
  --no-tgz-build \
  --tgz-path build/linux/dist/headless/p2premote-headless_1.6.4-<git-sha>_x86_64-linux-gnu.tar.gz
```

## 5. Linux Headless Docker aarch64

在 aarch64 Linux 主机或 aarch64 Linux 环境中执行：

```bash
./scripts/build-docker.sh -v 1.6.4
```

该命令会选择当前主机架构生成 aarch64 Headless 包，再构建 aarch64 Docker 镜像。Docker daemon 需要支持 aarch64；如果使用 x86_64 主机上的 QEMU/Buildx，必须先准备对应的模拟执行环境，并注意完整 Rust release 链接会明显变慢。

## 常用检查

```bash
file build/linux/dist/headless/*
docker image inspect p2premote/client:1.6.4
```

Linux 脚本会清理并重建 `build/linux/headless` 和 `build/linux/dist/headless`，请不要把这些目录中的临时文件当作源码提交。
