# 独立远程桌面 Engine 的桌面客户端集成

## 产品边界

- “远程桌面”始终启动独立原生窗口；Vue 页面只发命令、显示成功或精确错误，不传递视频帧、键鼠或剪贴板数据。
- WGVPN 和桌面会话是两层生命周期：隧道可以独立存在；只有用户点击内置远程桌面后才启动 Host/Controller。
- 不调用 mstc，不检查 Windows RDP Edition，不复制 RDP 地址，不自动切换 RDP/VNC/GDI/Raw 等方案。
- 单文件超过 100 MiB、视频背压丢弃旧帧、相同剪贴板不重发是明确策略；其余错误全部结构化失败。

## 信令顺序

```text
Vue                  Active Service            Passive Service            Host Engine        Controller Engine
 | StartDesktopSession      |                         |                         |                    |
 |------------------------->| validate WGVPN/context |                         |                    |
 |                          | DesktopStart(v1, id, secret, 39090)              |                    |
 |                          |------------------------>| validate tunnel/version|                    |
 |                          |                         | host-user-session       |                    |
 |                          |                         |------------------------>|                    |
 |                          |                         |<----------- HostReady --|                    |
 |                          |<-- DesktopHostReady ----|                         |                    |
 |                          | controller-user-session |                         |                    |
 |                          |--------------------------------------------------------------->|
 |                          |<-------------------------------- FirstFrame + Streaming --------|
 |<-- success (native) -----|                         |                         |                    |
```

任何一步失败即终止。HostReady 后 Controller 失败时，Active Service 发送 `DesktopStop`；该清理失败会与 Controller 原因合并返回。

## 本地进程管理

- `remote-engine-manager` 校验 Engine 路径与 SessionConfig，通过 stdin 一次性写配置，通过 stdout 读取 NDJSON 事件，stderr 独立排空。
- Host 必须在 30 秒内到达 `HostReady`；Controller 必须在 30 秒内完成首帧并到达 `Streaming`。
- 监督线程每 200 ms 响应停止，同时检查 Engine 退出和终态事件。
- 优雅停止期限为 5 秒；超时后终止并回收进程。启动中任一步失败也立即终止并回收，禁止孤儿进程。
- Service 停隧道、登出和整体退出前先停止所有关联 Engine。

## 图形 Session

- Windows：Service 运行于 LocalSystem。`*-user-session` 使用 WTS 活动 console token、用户环境和 `CreateProcessAsUserW` 在 `winsta0\\default` 创建真实 Engine，并继承管理管道。
- Linux：root Service 的 `*-user-session` 通过 logind 选择唯一活动本地 X11 Session，从同 UID 进程环境验证唯一 `DISPLAY/XAUTHORITY/XDG_RUNTIME_DIR`，校验文件所有者、执行 initgroups/setgid/setuid 后运行。Wayland、无 Session 或多 Session 均明确失败。
- macOS：当前 Service user-session 调度仍未接入；调用会返回 `service_session_launcher_unsupported`，不冒充支持。

## 安装资源

- Windows 安装包资源：`resources/p2premote-desktop-engine.exe`。
- Linux 安装包资源：`resources/p2premote-desktop-engine`，安装 Service 时复制到固定 resources 目录。
- `P2PREMOTE_DESKTOP_ENGINE_PATH` 只用于开发/测试覆盖二进制路径，不承载秘密。
- Engine 分平台脚本先生成到 Engine 仓库的 `artifacts/<platform-arch>` 固定目录；桌面客户端分平台打包脚本只复制对应产物。Tauri `build.rs` 不再隐式编译 Engine，缺失产物直接失败。
- CI 如需覆盖固定目录，只能用 `P2PREMOTE_DESKTOP_ENGINE_ARTIFACT` 指向精确文件，禁止目录扫描和跨架构复用。

## 关键实现位置

- Service Engine 生命周期：`core/src/runtime/desktop_engine.rs`
- WebSocket 桌面信令：`core/src/runtime/p2p_signal.rs`
- IPC 命令：`core/src/control.rs`、`core/src/runtime/ipc.rs`
- Tauri 命令：`src-tauri/src/commands/service.rs`
- Vue 原生窗口入口：`src/views/Devices.vue`
- 平台资源构建：`src-tauri/build.rs`、`src-tauri/tauri.*.conf.json`

## 尚需实机证明的门槛

1. 安装后的 Windows LocalSystem Service 能通过 WTS 启动 Host 和 Controller，并由 Job/生命线完整回收。
2. 安装后的 Ubuntu 18.04 root systemd Service 能自动降权启动两种角色。
3. 双向画面、鼠标、键盘、Unicode 文本剪贴板、文件剪贴板、连续多帧和断线重连。
4. Controller 错误只在客户端状态/UI 展示；Vue 永不创建远程画面窗口。
