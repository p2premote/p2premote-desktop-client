# 独立 p2premote-desktop 的桌面客户端集成

## 产品边界

- “远程桌面”始终启动独立原生窗口；Vue 页面只发命令、显示成功或精确错误，不传递视频帧、键鼠或剪贴板数据。
- WGVPN 和桌面会话是两层生命周期：隧道可以独立存在；只有用户点击内置远程桌面后才启动 Host/Controller。
- 不调用 mstc，不检查 Windows RDP Edition，不复制 RDP 地址，不自动切换 RDP/VNC/GDI/Raw 等方案。
- 新组件基于完整 RustDesk 捕获、输入、剪贴板、编解码和窗口生命周期；p2premote 不链接其源码。

## 信令顺序

```text
Vue                  Active Service            Passive Service            Host Engine        Controller Engine
 | StartDesktopSession      |                         |                         |                    |
 |------------------------->| validate WGVPN/control |                         |                    |
 |                          | DesktopStart(v4, id, secret, 39090)              |                    |
|                          |==== WGVPN health/control TCP ====================>| validate peer IP  |
 |                          |                         | session-helper host     |                    |
 |                          |                         |------------------------>|                    |
 |                          |                         |<---------- host_ready --|                    |
 |                          |<== DesktopReady =========|                         |                    |
 |                          | session-helper connect  |                         |                    |
 |                          |--------------------------------------------------------------->|
 |                          |<-------------------------------- FirstFrame + Streaming --------|
 |<-- success (native) -----|                         |                         |                    |
```

中心服务端只参与初始授权和打洞。隧道建立后，`DesktopStart/Ready/Failed/Stop/Stopped` 全部通过 WGVPN 内现有的 48082 健康控制长连接传输，不使用 WebSocket `p2p_notify`，也不依赖 `access_grant` 的有效期。任何一步失败即终止。HostReady 后 Controller 失败时，Active Service 发送 `DesktopStop`；该清理失败会与 Controller 原因合并返回。

## 本地进程管理

- Service 只启动 `p2premote-desktop-session-helper.exe -- host/connect ... --machine-readable`，逐行读取 stdout JSON，stderr 独立排空。
- Host 必须在 30 秒内到达 `host_ready`；Controller 必须在 30 秒内收到首帧并到达 `streaming`。
- 监督线程每 200 ms 响应停止，同时检查组件退出和终态事件；协议错误、进程退出及超时均返回稳定错误码。
- Helper 使用 Windows Job Object；Service 终止 Helper 时，当前用户 Session 中的主程序同步退出并被回收，不允许孤儿进程。
- Service 停隧道、登出和整体退出前先停止所有关联 Engine。

## 图形 Session

- Windows：Service 运行于 LocalSystem。Session Helper 只接受唯一 `WTSActive` 会话，使用用户 token、用户环境和 `CreateProcessAsUserW` 在 `winsta0\\default` 创建组件；没有活动会话或存在多个活动会话均明确失败。
- Linux/macOS：首期不提供新组件；调用明确返回 `desktop_platform_unsupported`，不运行旧 Engine 兜底。

## 安装资源

- 独立仓库固定产物目录：`remoteDesk/p2premote-desktop/dist/windows-x64-release`。
- Windows 安装包复制整个运行目录到 `resources/p2premote-desktop/`，不能只复制 exe。
- `P2PREMOTE_DESKTOP_SESSION_HELPER_PATH` 只用于开发/测试覆盖 Helper 精确路径。
- `P2PREMOTE_DESKTOP_ARTIFACT_DIR` 仅供 CI 覆盖固定产物目录；缺少主程序或 Helper 时构建立即失败。
- Linux/macOS 安装包不再包含旧自研 Engine。

## 关键实现位置

- Service Engine 生命周期：`core/src/runtime/desktop_engine.rs`
- 隧道内桌面控制信令：`core/src/tunnel_control.rs`、`core/src/health.rs`、`core/src/p2p.rs`
- IPC 命令：`core/src/control.rs`、`core/src/runtime/ipc.rs`
- Tauri 命令：`src-tauri/src/commands/service.rs`
- Vue 原生窗口入口：`src/views/Devices.vue`
- 平台资源构建：`src-tauri/build.rs`、`src-tauri/tauri.*.conf.json`

## 尚需实机证明的门槛

1. 安装后的 Windows LocalSystem Service 能通过 Helper 启动 Host 和 Controller，并由 Job 完整回收。
2. Windows 双机验证双向画面、鼠标、键盘、Unicode 文本剪贴板、文件剪贴板、连续多帧和断线重连。
3. Controller 错误只在客户端状态/UI 展示；Vue 永不创建远程画面窗口。
