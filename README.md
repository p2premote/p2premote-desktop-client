# p2pRemote Desktop Client

English | [简体中文](README_zh.md)

The p2pRemote desktop client provides P2P remote access, remote desktop control, and a WireGuard virtual LAN. Remote traffic flows directly between peers without passing through the server.

## Official Links

- Website: <https://www.p2premote.top>
- Downloads: <https://www.p2premote.top/#download>
- GitHub organization: <https://github.com/p2premote>
  - Android client: [p2premote-android-client](https://github.com/p2premote/p2premote-android-client)

## Overview

The p2pRemote desktop client can both initiate connections and accept them as a remote host. It supports connections between desktop clients and from Android to desktop clients (Desktop ↔ Desktop, Android → Desktop).

Remote data travels through a P2P tunnel. The NAT traversal engine establishes an AES-encrypted direct UDP tunnel, with WireGuard VPN running inside it. The server handles only account authentication, device management, and connection signaling.

For product documentation, see [docs/client-product-spec.md](docs/client-product-spec.md).

## Features

- P2P connections: establish encrypted tunnels across NAT using UDP hole punching and STUN, with TCP as a fallback.
- Virtual LAN: create a point-to-point WireGuard tunnel. Each peer receives a virtual IP address and can access the other through that address.
- Remote desktop: control a remote desktop using the embedded RustDeskTiny component.
- Remote assistance: connect using invite codes or one-time device connection passwords.
- LAN access: expose the local LAN subnet to the peer through the tunnel (subnet router).
- TCP service forwarding: access RDP, SSH, and other TCP services through the peer's virtual IP address.
- Web administration: manage the client in a browser. Initialization uses trust on first use (TOFU), with support for security codes and source IP allowlists.
- Speed tests: use the built-in iperf3 protocol implementation. See [vendor/riperf3](vendor/riperf3) for the code.
- System integration: background service, CLI, desktop notifications, startup at boot, and system tray.
- Interface: built with Vue 3 and Element Plus, with Simplified Chinese and English support and a dark theme.

## Supported Platforms

| Platform | Package |
| --- | --- |
| Windows 10 / 11 (a Win7-compatible package is also available) | NSIS installer |
| macOS (Intel and Apple Silicon) | Universal PKG installer |
| Linux desktop | DEB package (x86_64) |
| Headless Linux server | tar.gz / DEB / RPM / Docker image (x86_64 / aarch64) |

## Ports and Networking

### Local Listening Ports

| Port | Protocol | Default binding | Purpose |
| --- | --- | --- | --- |
| 48083 | TCP | `127.0.0.1`. Binds to `0.0.0.0` when a remote access IP allowlist is configured or TOFU initialization is pending | Browser-based web administration. Override the listening address with `P2PREMOTE_WEB_ADMIN_ADDR` |
| 41119 | TCP | `0.0.0.0` | Tunnel health checks and control traffic, accessed by the peer through the tunnel's virtual IP address (in-tunnel traffic) |
| 48082 | TCP | `0.0.0.0` | Health-check port for compatibility with older clients. Listens alongside the new port |
| 48084 | TCP | `0.0.0.0` | Built-in speed-test server (riperf3 protocol). Listens only during speed tests |
| 41118 | UDP | The Windows userspace engine binds to `127.0.0.1` (loopback) and receives data from the outer NAT traversal tunnel. Linux kernel WireGuard uses this port as its `ListenPort` | WireGuard data-plane port, exchanged with the peer during connection setup (51820 in older versions) |
| 21121 | TCP | — | RustDeskTiny's direct-connection listener. Remote control connects to this port on the peer's virtual IP address |
| Dynamic | UDP | — | The outer encrypted NAT traversal tunnel and probe sockets for each round use ephemeral ports assigned by the operating system |

The GUI communicates with the background service through Windows named pipes or Unix domain sockets, without using network ports.

### Outbound Connections

| Destination | Port | Purpose |
| --- | --- | --- |
| `cli.p2premote.top` | 443/TCP | Server APIs for account authentication, device management, and connection signaling |
| `www.p2premote.top` | 443/TCP | Update checks and website navigation |
| Public MQTT brokers (HiveMQ, EMQX, Mosquitto, and `mqtt.gonc.cc`) | 1883/TCP | Exchange WireGuard public keys and tunnel parameters with the peer over an encrypted channel during connection setup. Override the server list with `P2PREMOTE_MQTT_BROKERS` |
| STUN servers (`stun.gonc.cc`, `stun.hitv.com`, Cloudflare, Google, and Twilio) | UDP 3478 / 19302, TCP 80, UDP 53 | Detect the NAT type and discover the public address. Override the server list with `P2PREMOTE_STUN_SERVERS` |
| Peer public address | Dynamic UDP (TCP fallback) | Transfer data through the AES-encrypted outer tunnel after successful hole punching |

Once the tunnel is established, the local client can initiate connections to services on the peer's virtual IP address (in the `100.99.71.0/24` subnet):

- Health checks: 41119.
- Speed tests: 48084.
- Remote desktop: 21121.
- RDP, SSH, and other TCP services exposed by the user.

All of this traffic travels directly through the P2P tunnel without passing through the server.

## Repository Structure

```text
src/            Vue 3 + TypeScript frontend (Element Plus, Pinia, vue-i18n)
src-tauri/      Tauri shell, system tray, and packaging configuration (NSIS / macOS / icons)
core/           Rust core library: authentication, devices, P2P tunnels, WireGuard, web administration
service/ cli/ notifier/   Background service, CLI, and notification helper
scripts/        Platform build scripts (see BUILD.md)
packaging/      Linux packaging (Docker builder, deb installation scripts)
vendor/ third_party/      Bundled third-party code (riperf3, wireguard-tools)
```

## Building

Building requires Node.js, Rust, and the Tauri CLI. Place these two source dependencies in sibling directories:

- `../p2premote-punch-rs`: NAT traversal library.
- `../p2premote-wg-ffi`: WireGuard FFI.

The build scripts fetch the RustDeskTiny remote desktop component from release artifacts. See [BUILD.md](BUILD.md) for Linux cross-compilation and macOS signing and notarization instructions.

```powershell
# Windows (PowerShell)
.\scripts\build-windows.ps1 -v 1.12.0
```

```bash
# macOS (add --unsigned for internal testing without a certificate)
./scripts/build-macos.sh -v 1.12.0

# Linux: unified GUI and Headless build in four formats; run on Linux / WSL with only Docker required locally
./scripts/build-linux.sh -v 1.12.0
```

## Local Development

```bash
npm install
npm run tauri dev        # Desktop development mode (Vite + Tauri)
npm run build            # Frontend type checking and build
npx playwright test      # End-to-end tests
```

## Download and Install

Release packages: <https://www.p2premote.top/#download>
