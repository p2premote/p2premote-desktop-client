# 桌面客户端编译说明

本文档覆盖桌面客户端的 5 种交付产物。所有版本号都使用 `-v` 传入，格式为三段式 SemVer，例如 `1.6.4`。脚本会把当前 Git 短提交号追加到内部构建版本中。

## 产物总览

| 产物 | 架构 | 编译入口 |
| --- | --- | --- |
| Windows GUI 安装包 | 当前 Windows 主机架构（通常为 x64） | `scripts/build-windows.ps1` |
| Linux Headless 压缩包 | x86_64 | `scripts/build-linux-headless.sh` |
| Linux Headless 压缩包 | aarch64 | `scripts/build-linux-headless.sh` |
| Linux Headless DEB | amd64 / arm64 | `scripts/build-linux-headless.sh` |
| Linux Headless RPM | x86_64 / aarch64 | `scripts/build-linux-headless.sh` |
| Linux Headless Docker 镜像 | x86_64 | `scripts/build-docker.sh` |
| Linux Headless Docker 镜像 | aarch64 | `scripts/build-docker.sh` |

Linux Headless 图形安装入口明确支持 Deepin/UOS（DDE）和银河麒麟、
Ubuntu Kylin、openKylin（UKUI）的桌面版本，支持 x86_64
与 aarch64。用户完整解压压缩包后，可双击 `安装-p2pRemote.desktop`，
通过系统 Polkit 密码窗口完成安装；无桌面服务器仍使用
`sudo ./install-service.sh`。

## 1. Windows GUI

在 PowerShell 中执行：

```powershell
.\scripts\build-windows.ps1 -v 1.6.4
```

脚本会更新 Windows 客户端版本信息，然后执行 `npx tauri build`。NSIS 安装包通常位于：

```text
src-tauri\target\release\bundle\nsis\
```

脚本会把 NSIS 安装包规范化为 `p2pRemote_<version>-<git-sha>_x64-setup.exe`，文件名中的 `x64` 用于后台校验架构。

要求已安装 Node.js、Rust、Tauri CLI、Windows 编译工具链，并且相邻目录 `../p2premote-punch` 必须存在；脚本会把当前版本和该源码目录显式传给 Tauri 构建。任何必需工具、源码或资源缺失都会直接失败。

## 2. Linux Headless x86_64

必须在 x86_64 Linux 或 x86_64 WSL shell 中执行，不能从 Windows PowerShell 直接调用 Linux 脚本：

```bash
./scripts/build-linux-headless.sh -v 1.6.4
```

本机只需要 Docker：脚本会自动在 Debian 10 / glibc 2.28 基线的 builder 容器（`p2premote-linux-builder:glibc2.28-packages-v2-<arch>`）内完成全部编译，产物 glibc 依赖固定为 GLIBC_2.28，不受宿主机环境影响；宿主机工具链不参与编译。首次运行会自动构建 builder 镜像，工具链下载较慢时可通过 `--proxy http://127.0.0.1:<port>` 指定代理，脚本会自动把 WSL localhost 映射到 Docker 宿主机。也可以使用 `P2PREMOTE_BUILDER_PROXY` 或标准的 `HTTPS_PROXY`、`HTTP_PROXY` 环境变量。产物位于：

如果官方 Go 或 Node.js 下载站点连接不稳定，也可以分别设置 `P2PREMOTE_GO_DOWNLOAD_BASE` 和 `P2PREMOTE_NODE_DOWNLOAD_BASE` 覆盖下载根地址。下载命令默认强制 HTTP/1.1，并启用重试、断点续传和低速超时，避免 HTTP/2 中断后长时间卡住。

```text
build/linux/dist/headless/p2premote-headless_<version>-<git-sha>_x86_64-linux-gnu.tar.gz
build/linux/dist/headless/p2premote-headless_<version>-<git-sha>_amd64.deb
build/linux/dist/headless/p2premote-headless-<version>-1.<git-sha>.x86_64.rpm
```

## 3. Linux Headless aarch64

必须在 aarch64 Linux 主机或 aarch64 Linux 环境中执行同一个脚本：

```bash
./scripts/build-linux-headless.sh -v 1.6.4
```

产物位于：

```text
build/linux/dist/headless/p2premote-headless_<version>-<git-sha>_aarch64-linux-gnu.tar.gz
build/linux/dist/headless/p2premote-headless_<version>-<git-sha>_arm64.deb
build/linux/dist/headless/p2premote-headless-<version>-1.<git-sha>.aarch64.rpm
```

当前脚本根据 `uname -m` 选择架构；它不会在 x86_64 主机上自动把完整 Headless 客户端交叉编译成 aarch64。aarch64 包需在 aarch64 主机或环境上执行同一条命令，脚本会使用对应架构的 builder 容器（glibc2.28-packages-v2-arm64）。

builder 镜像基于 Debian 10 (buster) / glibc 2.28，内置 Node 22、Rust、Go 1.25 与 musl 工具链，产物（包括浏览器管理 UI）全部在容器内编译。基线选择依据：Node 22 官方二进制要求 GLIBC_2.28+（Ubuntu 18.04 / glibc 2.27 无法运行），Debian 10 是能承载完整工具链的最低 Debian/Ubuntu 版本；buster 已过 LTS，apt 源指向阿里云 debian-archive 冻结仓库。

Linux 构建脚本要求架构和编译器严格匹配；缺少目标架构工具链、产物不存在或产物架构不匹配时会直接失败，不会回退到宿主架构或其他 tar 包。

tar.gz、deb 和 rpm 都把程序、配置、日志及运行数据集中在 `/opt/p2premote`。包升级只替换 `resources` 并保留现有数据；卸载包时直接删除整个 `/opt/p2premote`，不保留用户配置或日志。systemd unit 按系统要求安装到 `/usr/lib/systemd/system`。

在 WSL 下可以直接从 `/mnt/c`、`/mnt/d` 等 Windows 映射目录启动构建。脚本会把 tar、deb、rpm 的中间打包目录放在 builder 容器的 `/tmp` 原生 Linux 文件系统中，仅将最终产物复制回工作区，避免 DrvFS 将 `DEBIAN` 等目录呈现为 `0777` 而被打包工具拒绝。

脚本自动完成容器编排：宿主机缺少镜像时先自动 `docker build`（可用 `P2PREMOTE_BUILDER_PROXY=http://<proxy>` 加速工具链下载），然后把整个 p2premote-all 挂载到 `/workspace`，并用命名卷缓存 cargo target/registry、npm 与 Go 模块和构建缓存，重复构建只做增量编译。挂载仓库的属主与容器 root 不同时 git 会拒绝操作，脚本已在容器内自动执行 `git config --global --add safe.directory '*'` 放行。

`P2PREMOTE_SKIP_WEB_BUILD=1 ./scripts/build-linux-headless.sh -v 1.6.4` 仍可用于复用已构建好的 `dist`（在容器内同样生效）：它只跳过前端编译，仍会重新编译 punch、wireguard、Rust service/CLI，并把已有的 `dist` 打入包中。

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
build/linux/dist/docker/p2premote-client_1.6.4_x86_64-linux-gnu.tar
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
