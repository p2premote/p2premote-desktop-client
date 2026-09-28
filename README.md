# p2pRemote Desktop Client

P2P remote access made simple: NAT-traversed direct connections with built-in remote desktop and a WireGuard-based virtual LAN — your remote traffic stays peer-to-peer.

p2pRemote 桌面客户端:P2P 远程访问 + 远程桌面 + WireGuard 虚拟局域网,数据面端到端直连,不经过服务端。

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
| macOS(Intel + Apple Silicon) | Universal DMG |
| Linux 桌面 | DEB / AppImage(x86_64) |
| Linux 无头服务器 | tar.gz / DEB / RPM / Docker 镜像(x86_64 / aarch64) |

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

**Platforms**: Windows 10/11 (plus a Win7 build), macOS universal DMG, Linux desktop (DEB / AppImage), headless Linux (tar.gz / DEB / RPM / Docker, x86_64 & aarch64).

**Build**: see [BUILD.md](BUILD.md). Requires Node.js, Rust, Tauri CLI and sibling checkouts `../p2premote-punch-rs` and `../p2premote-wg-ffi`.
