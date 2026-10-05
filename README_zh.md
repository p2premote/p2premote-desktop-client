# p2pRemote 桌面客户端

[English](README.md) | 简体中文

p2pRemote 桌面客户端提供 P2P 远程访问、远程桌面和 WireGuard 虚拟局域网。远程流量由两端直接传输，不经过服务端。

## 官方链接

- 官网：<https://www.p2premote.top>
- 下载页：<https://www.p2premote.top/#download>
- GitHub 组织：<https://github.com/p2premote>
  - Android 客户端：[p2premote-android-client](https://github.com/p2premote/p2premote-android-client)

## 项目简介

p2pRemote 桌面客户端既能发起连接，也能作为被控端接受连接。支持桌面端之间互连，以及 Android 端连接桌面端（Desktop ↔ Desktop、Android → Desktop）。

远程数据通过 P2P 隧道传输。NAT 穿透引擎建立 AES 加密的 UDP 直连隧道，WireGuard VPN 在隧道内运行。服务端只负责账号鉴权、设备管理和连接信令。

产品文档见 [docs/client-product-spec.md](docs/client-product-spec.md)。

## 主要功能

- P2P 直连：通过 UDP 打洞和 STUN 跨 NAT 建立加密隧道，TCP 作为后备方案。
- 虚拟局域网：建立点对点 WireGuard 隧道，两端各获得一个虚拟 IP，可通过虚拟 IP 互访。
- 远程桌面：内嵌 RustDeskTiny，支持远程控制。
- 远程协助：支持邀请码和设备一次性连接密码。
- 局域网互通：通过隧道向对端开放本机 LAN 网段（subnet router）。
- TCP 服务转发：通过对端虚拟 IP 访问 RDP、SSH 等 TCP 服务。
- Web 管理：可在浏览器中管理客户端，采用首次使用信任（TOFU）机制初始化。支持安全码和来源 IP 白名单。
- 测速：内置 iperf3 协议实现，代码见 [vendor/riperf3](vendor/riperf3)。
- 系统集成：支持后台服务、CLI、桌面通知、开机自启和系统托盘。
- 界面：使用 Vue 3 和 Element Plus，支持简体中文、English 和深色主题。

## 平台支持

| 平台 | 形态 |
| --- | --- |
| Windows 10 / 11（另提供 Win7 兼容包） | NSIS 安装包 |
| macOS（Intel 和 Apple Silicon） | Universal PKG 安装包 |
| Linux 桌面 | DEB 安装包（x86_64） |
| Linux 无头服务器 | tar.gz / DEB / RPM / Docker 镜像（x86_64 / aarch64） |

## 端口与网络

### 本机监听端口

| 端口 | 协议 | 默认绑定 | 用途 |
| --- | --- | --- | --- |
| 48083 | TCP | `127.0.0.1`。配置了远程访问 IP 白名单，或 TOFU 待初始化时，绑定 `0.0.0.0` | 浏览器 Web 管理端，可用环境变量 `P2PREMOTE_WEB_ADMIN_ADDR` 覆盖监听地址 |
| 41119 | TCP | `0.0.0.0` | 隧道健康检查和控制通信，对端通过隧道虚拟 IP 访问（隧道内流量） |
| 48082 | TCP | `0.0.0.0` | 兼容旧版本客户端的健康检查端口，与新端口同时监听 |
| 48084 | TCP | `0.0.0.0` | 内置测速服务端（riperf3 协议），仅在测速期间监听 |
| 41118 | UDP | Windows 用户态引擎绑定 `127.0.0.1`（回环地址），由外层打洞隧道投递数据；Linux 内核 WireGuard 将此端口用作 `ListenPort` | WireGuard 数据面端口，建立连接时与对端交换（旧版本为 51820） |
| 21121 | TCP | — | RustDeskTiny 的直连监听端口。发起远程控制时，连接对端虚拟 IP 的此端口 |
| 动态 | UDP | — | NAT 打洞的外层加密隧道和各轮探测 socket，使用操作系统分配的临时端口 |

GUI 与后台服务通过 Windows 命名管道或 Unix 域套接字通信，不占用网络端口。

### 对外连接（出站）

| 目标 | 端口 | 用途 |
| --- | --- | --- |
| `cli.p2premote.top` | 443/TCP | 账号认证、设备管理、连接信令等服务端 API |
| `www.p2premote.top` | 443/TCP | 版本更新检查、官网跳转 |
| 公共 MQTT broker（HiveMQ、EMQX、Mosquitto 和 `mqtt.gonc.cc`） | 1883/TCP | 建立连接时，通过加密通道与对端交换 WireGuard 公钥和隧道参数。可用 `P2PREMOTE_MQTT_BROKERS` 覆盖服务器列表 |
| STUN 服务器（`stun.gonc.cc`、`stun.hitv.com`、Cloudflare、Google 和 Twilio） | UDP 3478 / 19302、TCP 80、UDP 53 | 探测 NAT 类型和公网地址，可用 `P2PREMOTE_STUN_SERVERS` 覆盖服务器列表 |
| 对端公网地址 | 动态 UDP（TCP 作为后备方案） | 打洞成功后，通过 AES 加密的外层隧道传输数据 |

隧道建立后，本机可主动访问对端虚拟 IP（`100.99.71.0/24` 网段）上的服务：

- 健康检查：41119。
- 测速：48084。
- 远程桌面：21121。
- 用户自行开放的 RDP、SSH 等 TCP 服务。

这些流量均通过 P2P 隧道直接传输，不经过服务端。

## 仓库结构

```text
src/            Vue 3 + TypeScript 前端（Element Plus、Pinia、vue-i18n）
src-tauri/      Tauri 壳、系统托盘与打包配置（NSIS / macOS / 图标）
core/           Rust 核心库：认证、设备、P2P 隧道、WireGuard、Web 管理
service/ cli/ notifier/   后台服务、命令行与通知辅助程序
scripts/        各平台构建脚本（详见 BUILD.md）
packaging/      Linux 打包（Docker builder、deb 安装脚本）
vendor/ third_party/      内嵌第三方代码（riperf3、wireguard-tools）
```

## 构建

构建需要 Node.js、Rust 和 Tauri CLI。还需在相邻目录准备两份源码：

- `../p2premote-punch-rs`：NAT 穿透库。
- `../p2premote-wg-ffi`：WireGuard FFI。

构建脚本会从 Release 产物中获取远程桌面组件 RustDeskTiny。Linux 交叉编译、macOS 签名和公证的说明见 [BUILD.md](BUILD.md)。

```powershell
# Windows（PowerShell）
.\scripts\build-windows.ps1 -v 1.12.0
```

```bash
# macOS（无证书内部测试加 --unsigned）
./scripts/build-macos.sh -v 1.12.0

# Linux：统一构建 GUI 和 Headless 四种格式，在 Linux / WSL 中执行，本机只需 Docker
./scripts/build-linux.sh -v 1.12.0
```

## 本地开发

```bash
npm install
npm run tauri dev        # 桌面开发模式（Vite + Tauri）
npm run build            # 前端类型检查 + 构建
npx playwright test      # 端到端测试
```

## 下载安装

正式版安装包：<https://www.p2premote.top/#download>
