# WGVPN Windows 运行资源

Windows 客户端使用内嵌在 `p2premote-punch.dll` 中的 `wireguard-go`，不再调用外部
`wireguard.exe` 或 `wg.exe`。

## 资源职责

| 文件 | 职责 |
|---|---|
| `p2premote-punch.dll` | gonc 打洞、加密 UDP 数据面、用户态 WireGuard、gVisor 被动代理 |
| `wintun.dll` | Windows 主动端的系统 TUN 驱动，供普通应用透明访问虚拟 IP/LAN |
| `p2premote-service.exe` | 登录、信令、隧道状态机、健康监测和资源生命周期 |

被动端访问本机和 LAN 时由 gVisor 用户态协议栈终止并代理 TCP/UDP；只有存在主动
Peer 时才创建 Wintun 适配器。WireGuard UDP listen socket 只绑定 `127.0.0.1`，公网
数据始终经过 gonc。

## 打包结构

```text
resources/
├── p2premote-service.exe
├── p2premote-cli.exe
├── p2premote-punch.dll
├── wintun.dll
└── web/
```

service 重启后不会恢复进程内会话；磁盘 session 仅用于识别和清理过期状态。

## 许可

- `wireguard-go`：MIT。
- `gVisor`：Apache-2.0。
- `wintun.dll`：WireGuard 项目提供的预编译驱动，随发布包保留其许可文件与来源记录。
