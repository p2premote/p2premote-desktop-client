# p2pRemote Desktop Client

P2P remote access made simple: NAT-traversed direct connections with built-in remote desktop and a WireGuard-based virtual LAN — your remote traffic stays peer-to-peer.

p2pRemote 桌面客户端:P2P 远程访问 + 远程桌面 + WireGuard 虚拟局域网,数据面端到端直连,不经过服务端。

## 官方链接

- 官网：<https://www.p2premote.top>
- 下载页：<https://www.p2premote.top/#download>
- GitHub 组织：<https://github.com/p2premote>
  - Android 客户端：[p2premote-android-client](https://github.com/p2premote/p2premote-android-client)

## 项目简介

p2pRemote 桌面客户端是 p2pRemote P2P 远程访问产品中功能最完整的端:既能发起连接,也能作为被控端接受连接(Desktop ↔ Desktop、Android → Desktop)。

数据面为纯 P2P:外层由 NAT 穿透引擎建立 AES 加密的 UDP 直连隧道,内层运行 WireGuard VPN;服务端只负责账号鉴权、设备管理与建链信令,远程流量不经过服务端。产品文档见 [docs/client-product-spec.md](docs/client-product-spec.md)。

## 主要功能

- **P2P 直连**:UDP 打洞 + STUN,TCP 作为兜底,跨 NAT 建立加密隧道
- **虚拟局域网**:点对点 WireGuard 隧道,两端各获得虚拟 IP,像局域网一样互访
- **远程桌面**:内嵌 RustDeskTiny,开箱即用的远程控制
- **远程协助**:邀请码 / 设备一次性连接密码
- **局域网互通**:可将本机 LAN 网段通过隧道暴露给对端(subnet router)
- **TCP 服务转发**:经隧道虚拟 IP 直接访问对端 RDP、SSH 等 TCP 服务
- **浏览器 Web 管理**:首次使用信任(TOFU)初始化,支持安全码与来源 IP 白名单
- **测速**:内置 iperf3 协议实现([vendor/riperf3](vendor/riperf3))
- **系统集成**:后台服务、CLI、桌面通知、开机自启、系统托盘
- **界面**:Vue 3 + Element Plus,简体中文 / English 双语,深色主题

## 平台支持

| 平台 | 形态 |
| --- | --- |
| Windows 10 / 11(另提供 Win7 兼容包) | NSIS 安装包 |
| macOS(Intel + Apple Silicon) | Universal PKG 安装包 |
| Linux 桌面 | DEB 安装包(x86_64) |
| Linux 无头服务器 | tar.gz / DEB / RPM / Docker 镜像(x86_64 / aarch64) |

## 端口与网络

### 本机监听端口

| 端口 | 协议 | 默认绑定 | 用途 |
| --- | --- | --- | --- |
| 48083 | TCP | `127.0.0.1`;配置了远程访问 IP 白名单或首次使用信任(TOFU)待初始化时绑 `0.0.0.0` | 浏览器 Web 管理端,可用环境变量 `P2PREMOTE_WEB_ADMIN_ADDR` 覆盖 |
| 41119 | TCP | `0.0.0.0` | 隧道健康/控制面,对端经隧道虚拟 IP 访问(隧道内流量) |
| 48082 | TCP | `0.0.0.0` | 旧版本客户端的健康口兼容监听,与新端口同时开启 |
| 48084 | TCP | `0.0.0.0` | 内置测速(riperf3 协议)服务端,仅在测速进行期间监听 |
| 41118 | UDP | Windows 用户态引擎绑 `127.0.0.1`(loopback),由打洞外层隧道投递;Linux 内核 WireGuard 用作 `ListenPort` | WireGuard 数据面端口,建链时与对端交换(旧版本为 51820) |
| 21121 | TCP | — | 内置远程桌面 RustDeskTiny 自有的直连监听;发起控制时连接的是对端虚拟 IP 的该端口 |
| 动态 | UDP | — | NAT 打洞的外层加密隧道与各轮探测 socket,使用操作系统分配的临时端口 |

GUI 与后台服务之间的通信走 Windows named pipe / Unix domain socket,不占用网络端口。

### 对外连接(出站)

| 目标 | 端口 | 用途 |
| --- | --- | --- |
| `cli.p2premote.top` | 443/TCP | 账号认证、设备管理、连接信令等服务端 API |
| `www.p2premote.top` | 443/TCP | 版本更新检查、官网跳转 |
| 公共 MQTT broker(hivemq / emqx / mosquitto / `mqtt.gonc.cc`) | 1883/TCP | 建链时在加密通道中与对端交换 WireGuard 公钥与隧道参数,可用 `P2PREMOTE_MQTT_BROKERS` 覆盖 |
| STUN 服务器(`stun.gonc.cc`、`stun.hitv.com`、Cloudflare、Google、Twilio) | UDP 3478 / 19302,TCP 80,UDP 53 | NAT 类型探测与公网地址发现,可用 `P2PREMOTE_STUN_SERVERS` 覆盖 |
| 对端公网地址 | 动态 UDP(兜底 TCP) | 打洞成功后的 AES 加密外层隧道数据面 |

隧道建立后,本机还可主动访问对端虚拟 IP(`100.99.71.0/24` 网段)上的服务:健康面 41119、测速 48084、远程桌面 21121,以及用户自行开放的 RDP、SSH 等 TCP 服务。远程流量全部在 P2P 隧道内直连,不经过服务端。

## 仓库结构

```text
src/            Vue 3 + TypeScript 前端(Element Plus、Pinia、vue-i18n)
src-tauri/      Tauri 壳、系统托盘与打包配置(NSIS / macOS / 图标)
core/           Rust 核心库:认证、设备、P2P 隧道、WireGuard、Web 管理
service/ cli/ notifier/   后台服务、命令行与通知辅助程序
scripts/        各平台构建脚本(详见 BUILD.md)
packaging/      Linux 打包(Docker builder、deb 安装脚本)
vendor/ third_party/      内嵌第三方代码(riperf3、wireguard-tools)
```

## 构建

构建依赖 Node.js、Rust、Tauri CLI,以及相邻目录的两份源码依赖:`../p2premote-punch-rs`(NAT 穿透库)与 `../p2premote-wg-ffi`(WireGuard FFI);远程桌面组件 RustDeskTiny 由构建脚本从 Release 产物获取。Linux 交叉编译、macOS 签名公证等完整说明见 [BUILD.md](BUILD.md)。

```powershell
# Windows(PowerShell)
.\scripts\build-windows.ps1 -v 1.12.0
```

```bash
# macOS(无证书内部测试加 --unsigned)
./scripts/build-macos.sh -v 1.12.0

# Linux:GUI + Headless 四格式统一构建,在 Linux / WSL 中执行,本机只需 Docker
./scripts/build-linux.sh -v 1.12.0
```

## 本地开发

```bash
npm install
npm run tauri dev        # 桌面开发模式(Vite + Tauri)
npm run build            # 前端类型检查 + 构建
npx playwright test      # 端到端测试
```

## 下载安装

正式版安装包:<https://www.p2premote.top/#download>

## English

p2pRemote is a P2P remote-access product; this desktop client can both initiate and receive connections. The data plane is pure peer-to-peer: an AES-encrypted UDP tunnel established by NAT traversal, with WireGuard running inside. The coordination server only handles authentication, device management, and connection signaling — remote traffic never touches it.

**Features**: UDP hole punching with TCP fallback · WireGuard virtual LAN with virtual IPs · embedded remote desktop (RustDeskTiny) · invite codes / one-time device passwords · LAN subnet exposure · TCP service forwarding · browser-based web admin · iperf3-based speed test · system service, CLI and notifier · bilingual (简体中文 / English) UI with dark theme.

**Platforms**: Windows 10/11 (plus a Win7 build), macOS universal PKG installer, Linux desktop (DEB), headless Linux (tar.gz / DEB / RPM / Docker, x86_64 & aarch64).

**Ports (inbound)**: 48083/TCP web admin (loopback by default, 0.0.0.0 when remote access is allowed) · 41119/TCP + 48082/TCP tunnel health/control (legacy compat) · 48084/TCP speed-test server (during tests only) · 41118/UDP WireGuard data plane (loopback userspace engine on Windows) · 21121/TCP is RustDeskTiny's own listener. GUI↔service IPC uses named pipes / Unix sockets.

**Outbound**: 443/TCP to `cli.p2premote.top` (API/signaling) and `www.p2premote.top` (updates) · 1883/TCP public MQTT brokers for the encrypted key exchange · STUN probes (UDP 3478/19302, TCP 80, UDP 53) · dynamic UDP (TCP fallback) to the peer for the encrypted data plane. See the 端口与网络 section above for details.

**Build**: see [BUILD.md](BUILD.md). Requires Node.js, Rust, Tauri CLI and sibling checkouts `../p2premote-punch-rs` and `../p2premote-wg-ffi`.
