# 桌面客户端编译说明

本文档覆盖桌面客户端的 Windows 交付产物，以及 Linux 的统一四格式交付。所有版本号都使用 `-v` 传入，格式为三段式 SemVer，例如 `1.6.4`。Linux 统一脚本会把当前 Git 短提交号追加到内部构建版本中。

## 产物总览

| 产物 | 架构 | 编译入口 |
| --- | --- | --- |
| Windows GUI 安装包 | 当前 Windows 主机架构（通常为 x64） | `scripts/build-windows.ps1` |
| macOS GUI PKG | Universal（x86_64 + arm64） | `scripts/build-macos.sh` |
| Linux GUI DEB | x86_64 | `scripts/build-linux-gui.sh -v <version>` |
| Linux Headless 裸压缩包、DEB、RPM、Docker 镜像 tar | x86_64 / aarch64 | `scripts/build-linux.sh` |

编译缓存统一放在 `target/<构建目标>/`，发布物统一放在 `artifacts/<发布目标>/`。`target` 可随时清理，不再保存需要交付的安装包；`artifacts` 不参与 Cargo 增量编译，也不会被前端 Vite 清理。主要目录如下：

```text
target/windows-x64/             artifacts/windows-x64/
target/windows-win7-x64/        artifacts/windows-win7-x64/
target/linux-gui-x64/           artifacts/linux-gui-x64/
target/linux-headless-amd64/    artifacts/linux-headless/
target/linux-headless-arm64/    artifacts/linux-headless/
target/linux-docker-amd64/      artifacts/linux-docker/
target/linux-docker-arm64/      artifacts/linux-docker/
target/macos-universal/         artifacts/macos-universal/
```

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

脚本会更新 Windows 客户端版本信息，然后执行 `npx tauri build`。Win10 与 Win7 的 Cargo 缓存分别位于 `target/windows-x64` 和 `target/windows-win7-x64`，最终 NSIS 安装包位于：

```text
artifacts\windows-x64\
```

Win7 安装包输出到 `artifacts\windows-win7-x64\`。普通 Windows 包保持 `p2pRemote_<version>-<git-sha>_x64-setup.exe` 命名；Win7 包使用 `p2pRemote_<version>-<git-sha>_windows-win7-x64-setup.exe`，不同兼容目标不会互相覆盖。

要求已安装 Node.js、Rust、Tauri CLI、Windows 编译工具链，并且相邻目录 `../p2premote-punch-rs` 与 `../p2premote-wg-ffi` 必须存在；Rust Punch 由 Cargo 以源码依赖集成，脚本只构建独立的 WireGuard FFI。任何必需工具、源码或资源缺失都会直接失败。

RustDeskTiny 引擎安装包使用预编译产物，不进 git：本地 `src-tauri/resources/RustDeskTiny-install.exe` 已存在时直接复用，缺失时（如 CI）按 `scripts/rustdesktiny-artifacts.env` 钉版从 GitHub Release 自动下载并校验 SHA-256；也可用 `-RustDeskTinyInstaller` 或环境变量 `RUSTDESK_TINY_INSTALLER` 指定其他安装包。Win7 包使用的 `RustDeskTinyLegacy-install.exe` 来自独立的 RustDeskTinyLegacy 仓库 Release，遵循同样的"本地优先、缺失自动下载"策略。

## 2. macOS GUI

正式发布必须在 macOS 主机配置 Developer ID Application 证书与 Apple 公证凭据后执行：

```bash
export APPLE_SIGNING_IDENTITY='Developer ID Application: ...'
export APPLE_NOTARY_KEYCHAIN_PROFILE='p2premote-notary'
./scripts/build-macos.sh -v 1.11.2
```

脚本会构建 x86_64 与 arm64 的 service、CLI、WireGuard 数据面动态库（`libp2premote-wg.dylib`）及 Tauri GUI，合并为 Universal App，随后签名、公证并打包生成 `artifacts/macos-universal/p2pRemote_<version>_macos-universal.pkg`。编译缓存位于 `target/macos-universal`。没有发布证书的内部测试机可使用 `--unsigned` 生成 ad-hoc 签名、未经公证的 PKG；该产物只用于测试，不应对外分发：

```bash
./scripts/build-macos.sh -v 1.11.2 --no-sccache --unsigned
```

与 Windows/Linux 一致，Punch/Exchange 由 `../p2premote-punch-rs` 以 Cargo 源码依赖集成，Go 侧只从 `../p2premote-wg-ffi` 构建 wgonly 的 userspace WireGuard 数据面（产物 `libp2premote-wg.dylib`，与 Windows 的 `p2premote-wg.dll` 同源同参数）；相邻目录这两个仓库必须存在，不再依赖 Go 版 `p2premote-punch` 老库。

macOS 包内嵌 Universal `RustDeskTiny.app`。首次远程控制前，用户需要在“系统设置 → 隐私与安全性”中为 RustDeskTiny 授予屏幕录制、辅助功能和输入监控权限。

RustDeskTiny 不在本机编译：与 Windows 的 `RustDeskTiny-install.exe`、Linux 的 `RustDeskTiny.deb` 一样使用预编译产物。两个单架构压缩包在 `src-tauri/resources/` 已存在时直接复用，缺失时（如 CI）按 `scripts/rustdesktiny-artifacts.env` 钉版从 GitHub Release 自动下载并校验 SHA-256；构建脚本随后自动解包并用 lipo 合成 Universal app。这些产物不进 git。

需要复用已合成的 app 时，可通过 `RUSTDESK_TINY_APP` 指向该 `RustDeskTiny.app` 跳过合并。由于不再编译 RustDeskTiny，macOS 构建机不需要完整 Xcode、Flutter 与 vcpkg，只需 Rust、Go 1.20+、Node.js 与 Command Line Tools。

## 3. Linux Headless 四格式统一构建

Linux GUI 与 Headless 共用唯一一套 Debian 10 / glibc 2.28 编译镜像。可预先创建镜像：

```bash
./scripts/make_compile_image.sh --proxy http://<内网代理地址>:<端口>
```

正常编译脚本会先检查镜像；镜像不存在时会自动调用该脚本创建，不需要手工执行。
GUI 与 Headless 不再分别维护 Ubuntu 18 和 Debian 10 镜像。
当前 x64 编译镜像为 `p2premote-linux-compile:debian10-rust1.77-v1-amd64`，
arm64 使用同一标签前缀的 `-arm64` 镜像。镜像创建入口统一为 `make_compile_image.sh`。
GUI 构建入口为 `build-linux-gui.sh`，由上述 Debian 10 镜像执行。

GUI 构建不会编译 RustDeskTiny：`build-linux-gui.sh` 启动容器前检查 `src-tauri/resources/RustDeskTiny.deb`，本地已有直接复用，缺失时按 `scripts/rustdesktiny-artifacts.env` 钉版自动下载并校验 SHA-256（该产物不进 git），然后执行：

```bash
./scripts/build-linux-gui.sh -v 1.12.1
```

GUI 编译完成后，打包脚本会解开 RustDeskTiny deb，将它的数据文件、运行依赖与服务安装逻辑合并进
`artifacts/linux-gui-x64/p2premote_<version>-<commit-id>_amd64.deb`，AppImage 使用相同的版本与 commit id 后缀。GUI 与 Headless 仍输出到不同目录；
Headless 包不包含 RustDeskTiny。需要临时使用其他文件名时，可通过 `RUSTDESK_TINY_DEB` 指定。
GUI 的设备详情上报 `<version>-<commit-id>`；deb 的包版本仍为 `<version>`。

必须在目标架构的 Linux 或 WSL shell 中执行，不能从 Windows PowerShell 直接调用 Linux 脚本：

```bash
./scripts/build-linux.sh -v 1.6.4
```

本机只需要 Docker：脚本会自动在 Debian 10 builder 容器内构建。裸机 headless 与 Docker 使用不同的 `P2PREMOTE_RELEASE_TARGET` 独立编译 service/CLI；Docker 不再复用 headless tar。两种构建共享依赖下载缓存，连续构建时复用已经生成的前端。首次运行会自动构建 builder 镜像，工具链下载较慢时可通过 `--proxy http://127.0.0.1:<port>` 指定代理，脚本会自动把 WSL localhost 映射到 Docker 宿主机。也可以使用 `P2PREMOTE_BUILDER_PROXY` 或标准的 `HTTPS_PROXY`、`HTTP_PROXY` 环境变量。

如果官方 Go 或 Node.js 下载站点连接不稳定，也可以分别设置 `P2PREMOTE_GO_DOWNLOAD_BASE` 和 `P2PREMOTE_NODE_DOWNLOAD_BASE` 覆盖下载根地址。下载命令默认强制 HTTP/1.1，并启用重试、断点续传和低速超时，避免 HTTP/2 中断后长时间卡住。

```text
artifacts/linux-headless/p2premote-headless_<version>-<git-sha>_x86_64-linux-gnu.tar.gz
artifacts/linux-headless/p2premote-headless_<version>-<git-sha>_amd64.deb
artifacts/linux-headless/p2premote-headless-<version>-1.<git-sha>.x86_64.rpm
artifacts/linux-docker/p2premote-headless-docker_<version>-<git-sha>_x86_64-linux-gnu.tar
```

镜像默认标签为 `p2premote/client:<version>`，可通过 `--tag` 覆盖。aarch64 构建会把文件名中的架构替换为 `aarch64-linux-gnu`、`arm64` 和 `aarch64`。

可以只构建其中一种发布形式：

```bash
./scripts/build-linux.sh -v 1.6.4 --headless-only
./scripts/build-linux.sh -v 1.6.4 --docker-only
```

`_build-linux-headless.sh` 生成 tar.gz、deb、rpm；`_build-linux-docker.sh` 独立编译并生成镜像 tar；完整发布流程统一使用 `build-linux.sh`。

### aarch64

可以在 aarch64 Linux 环境中执行同一条命令；在 x86_64 WSL Debian 中需先注册 Docker/QEMU 的 arm64 模拟器，然后指定目标架构：

```bash
./scripts/build-linux.sh -v 1.6.4 --arch aarch64
```

不指定 `--arch` 时脚本仍按 `uname -m` 选择宿主架构；指定 `--arch aarch64` 后，脚本会给 builder build/run 和最终 Docker build 同时传入 `linux/arm64`，由 Docker/QEMU 执行完整编译。Docker daemon 需要先注册 QEMU arm64 模拟器。

builder 镜像基于 Debian 10 (buster) / glibc 2.28，内置 Node 22、Rust、Go 1.25 与 musl 工具链，产物（包括浏览器管理 UI）全部在容器内编译。基线选择依据：Node 22 官方二进制要求 GLIBC_2.28+（Ubuntu 18.04 / glibc 2.27 无法运行），Debian 10 是能承载完整工具链的最低 Debian/Ubuntu 版本；buster 已过 LTS，apt 源指向阿里云 debian-archive 冻结仓库。

Linux 构建脚本要求架构和编译器严格匹配；缺少目标架构工具链、产物不存在或产物架构不匹配时会直接失败，不会回退到宿主架构或其他 tar 包。

tar.gz、deb 和 rpm 都把程序、配置、日志及运行数据集中在 `/opt/p2premote`。包升级只替换 `resources` 并保留现有数据；卸载包时直接删除整个 `/opt/p2premote`，不保留用户配置或日志。systemd unit 安装到 `/lib/systemd/system`。

在 WSL 下可以直接从 `/mnt/c`、`/mnt/d` 等 Windows 映射目录启动构建。脚本会把 tar、deb、rpm 的中间打包目录放在 builder 容器的 `/tmp` 原生 Linux 文件系统中，仅将最终产物复制回工作区，避免 DrvFS 将 `DEBIAN` 等目录呈现为 `0777` 而被打包工具拒绝。

脚本自动完成容器编排：宿主机缺少镜像时先自动 `docker build`（可用 `P2PREMOTE_BUILDER_PROXY=http://<proxy>` 加速工具链下载），然后把整个 p2premote-all 挂载到 `/workspace`，并用命名卷缓存 cargo target/registry、npm 与 Go 模块和构建缓存，重复构建只做增量编译。挂载仓库的属主与容器 root 不同时 git 会拒绝操作，脚本已在容器内自动执行 `git config --global --add safe.directory '*'` 放行。

`P2PREMOTE_SKIP_WEB_BUILD=1 ./scripts/_build-linux-headless.sh -v 1.6.4` 仍可用于复用已构建好的前端 `dist`（在容器内同样生效）：它只跳过前端编译，仍会重新编译 punch、wireguard、Rust service/CLI，并把已有的前端 `dist` 打入包中；最终发布物仍写入 `artifacts/linux-headless`。

## 常用检查

```bash
file artifacts/linux-headless/*
docker image inspect p2premote/client:1.6.4
```

Linux 脚本只会替换当前版本和目标架构的构建产物，并保留其他版本及架构的包；请不要把这些目录中的生成文件当作源码提交。
