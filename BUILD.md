# 桌面客户端编译说明

本文档覆盖桌面客户端的 Windows 交付产物，以及 Linux 的统一四格式交付。所有版本号都使用 `-v` 传入，格式为三段式 SemVer，例如 `1.6.4`。Linux 统一脚本会把当前 Git 短提交号追加到内部构建版本中。

## 产物总览

| 产物 | 架构 | 编译入口 |
| --- | --- | --- |
| Windows GUI 安装包 | 当前 Windows 主机架构（通常为 x64） | `scripts/build-windows.ps1` |
| Linux Headless 裸压缩包、DEB、RPM、Docker 镜像 tar | x86_64 / aarch64 | `scripts/build-linux.sh` |

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

## 2. Linux Headless 四格式统一构建

必须在目标架构的 Linux 或 WSL shell 中执行，不能从 Windows PowerShell 直接调用 Linux 脚本：

```bash
./scripts/build-linux.sh -v 1.6.4
```

本机只需要 Docker：脚本会自动在 Debian 10 / glibc 2.28 基线的 builder 容器内完成唯一一次前端、Go、Rust 编译，宿主机工具链不参与编译；同一份 headless 产物随后派生出 tar.gz、deb、rpm，并复用 tar.gz 构建 Docker 镜像后导出 tar。首次运行会自动构建 builder 镜像，工具链下载较慢时可通过 `--proxy http://127.0.0.1:<port>` 指定代理，脚本会自动把 WSL localhost 映射到 Docker 宿主机。也可以使用 `P2PREMOTE_BUILDER_PROXY` 或标准的 `HTTPS_PROXY`、`HTTP_PROXY` 环境变量。

如果官方 Go 或 Node.js 下载站点连接不稳定，也可以分别设置 `P2PREMOTE_GO_DOWNLOAD_BASE` 和 `P2PREMOTE_NODE_DOWNLOAD_BASE` 覆盖下载根地址。下载命令默认强制 HTTP/1.1，并启用重试、断点续传和低速超时，避免 HTTP/2 中断后长时间卡住。

```text
build/linux/dist/headless/p2premote-headless_<version>-<git-sha>_x86_64-linux-gnu.tar.gz
build/linux/dist/headless/p2premote-headless_<version>-<git-sha>_amd64.deb
build/linux/dist/headless/p2premote-headless-<version>-1.<git-sha>.x86_64.rpm
build/linux/dist/docker/p2premote-client_<version>-<git-sha>_x86_64-linux-gnu.tar
```

镜像默认标签为 `p2premote/client:<version>`，可通过 `--tag` 覆盖。aarch64 构建会把文件名中的架构替换为 `aarch64-linux-gnu`、`arm64` 和 `aarch64`。

如果只需要复用已经生成的 headless 压缩包构建镜像，可使用兼容参数：

```bash
./scripts/build-linux.sh -v 1.6.4 --no-tgz-build \
  --tgz-path build/linux/dist/headless/p2premote-headless_1.6.4-<git-sha>_x86_64-linux-gnu.tar.gz
```

`build-linux-headless.sh` 仍保留为只生成 tar.gz、deb、rpm 的底层构建入口；新发布流程统一使用 `build-linux.sh`。

### aarch64

可以在 aarch64 Linux 环境中执行同一条命令；在 x86_64 WSL Debian 中需先注册 Docker/QEMU 的 arm64 模拟器，然后指定目标架构：

```bash
./scripts/build-linux.sh -v 1.6.4 --arch aarch64
```

不指定 `--arch` 时脚本仍按 `uname -m` 选择宿主架构；指定 `--arch aarch64` 后，脚本会给 builder build/run 和最终 Docker build 同时传入 `linux/arm64`，由 Docker/QEMU 执行完整编译。Docker daemon 需要先注册 QEMU arm64 模拟器。

builder 镜像基于 Debian 10 (buster) / glibc 2.28，内置 Node 22、Rust、Go 1.25 与 musl 工具链，产物（包括浏览器管理 UI）全部在容器内编译。基线选择依据：Node 22 官方二进制要求 GLIBC_2.28+（Ubuntu 18.04 / glibc 2.27 无法运行），Debian 10 是能承载完整工具链的最低 Debian/Ubuntu 版本；buster 已过 LTS，apt 源指向阿里云 debian-archive 冻结仓库。

Linux 构建脚本要求架构和编译器严格匹配；缺少目标架构工具链、产物不存在或产物架构不匹配时会直接失败，不会回退到宿主架构或其他 tar 包。

tar.gz、deb 和 rpm 都把程序、配置、日志及运行数据集中在 `/opt/p2premote`。包升级只替换 `resources` 并保留现有数据；卸载包时直接删除整个 `/opt/p2premote`，不保留用户配置或日志。systemd unit 按系统要求安装到 `/usr/lib/systemd/system`。

在 WSL 下可以直接从 `/mnt/c`、`/mnt/d` 等 Windows 映射目录启动构建。脚本会把 tar、deb、rpm 的中间打包目录放在 builder 容器的 `/tmp` 原生 Linux 文件系统中，仅将最终产物复制回工作区，避免 DrvFS 将 `DEBIAN` 等目录呈现为 `0777` 而被打包工具拒绝。

脚本自动完成容器编排：宿主机缺少镜像时先自动 `docker build`（可用 `P2PREMOTE_BUILDER_PROXY=http://<proxy>` 加速工具链下载），然后把整个 p2premote-all 挂载到 `/workspace`，并用命名卷缓存 cargo target/registry、npm 与 Go 模块和构建缓存，重复构建只做增量编译。挂载仓库的属主与容器 root 不同时 git 会拒绝操作，脚本已在容器内自动执行 `git config --global --add safe.directory '*'` 放行。

`P2PREMOTE_SKIP_WEB_BUILD=1 ./scripts/build-linux-headless.sh -v 1.6.4` 仍可用于复用已构建好的 `dist`（在容器内同样生效）：它只跳过前端编译，仍会重新编译 punch、wireguard、Rust service/CLI，并把已有的 `dist` 打入包中。

## 常用检查

```bash
file build/linux/dist/headless/*
docker image inspect p2premote/client:1.6.4
```

Linux 脚本只会替换当前版本和目标架构的构建产物，并保留其他版本及架构的包；请不要把这些目录中的生成文件当作源码提交。
