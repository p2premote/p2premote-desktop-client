# 独立 RustDeskTiny 的桌面客户端集成

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
 |                          | DesktopStart(v5, id, 39090)                      |                    |
|                          |==== WGVPN health/control TCP ====================>|                    |
|                          |                         | protected service IPC  |                    |
 |                          |                         |------------------------>|                    |
 |                          |                         |<---------- host_ready --|                    |
 |                          |<== DesktopReady =========|                         |                    |
 |                          | direct executable connect                       |                    |
 |                          |--------------------------------------------------------------->|
 |<-- launched (native) ----|                         |                         |                    |
```

中心服务端只参与初始授权和打洞。隧道建立后，桌面启动通知通过 WGVPN 内现有的 48082 健康控制长连接传输。该协议不传递密码或自定义会话令牌；连接认证完全由 RustDeskTiny 的 RustDesk 临时密码或长期密码完成。

## 本地进程管理

- Host 通过同一份 `RustDeskTiny.exe service-host ...` 经 RustDesk 受保护 Service IPC 启动；Controller 直接运行 `RustDeskTiny.exe connect ...`。不再使用自研 Session Supervisor。
- Host 命令确认 Service 已在指定 IP 监听后退出；Controller 使用脱离进程方式启动。
- p2pRemote 不保存子进程句柄、不读取长期状态输出、不轮询、不随隧道或自身退出而终止 RustDeskTiny。
- RustDeskTiny 自己负责窗口、连接、Windows Service、Session 迁移和崩溃恢复。
- 只有升级、卸载或替换运行文件时，安装流程才停止 RustDeskTiny。

## 图形 Session

- Windows：`RustDeskTinyService` 运行于 LocalSystem，直接复用 RustDesk 的 Session 枚举、`launch_server` 和登录/锁屏/RDP Session 迁移逻辑。
- Linux/macOS：首期不提供新组件；调用明确返回 `desktop_platform_unsupported`，不运行旧 Engine 兜底。

## 安装资源

- Windows 安装包复制整个运行目录到 `resources/RustDeskTiny/`，不能只复制 exe。
- `RUSTDESK_TINY_PATH` 只用于开发/测试覆盖桌面可执行文件精确路径。
- `RUSTDESK_TINY_ARTIFACT_DIR` 指向独立发布包的解压目录；构建不引用相邻源码仓库。
- Linux/macOS 安装包不再包含旧自研 Engine。

## 关键实现位置

- 一次性命令行启动适配：`core/src/runtime/desktop_engine.rs`
- 隧道内桌面控制信令：`core/src/tunnel_control.rs`、`core/src/health.rs`、`core/src/p2p.rs`
- IPC 命令：`core/src/control.rs`、`core/src/runtime/ipc.rs`
- Tauri 命令：`src-tauri/src/commands/service.rs`
- Vue 原生窗口入口：`src/views/Devices.vue`
- 平台资源构建：`src-tauri/build.rs`、`src-tauri/tauri.*.conf.json`

## 尚需实机证明的门槛

1. 安装后的 Windows LocalSystem 双服务能够保持相同启动类型，并由 RustDesk Service 在登录界面、锁屏、控制台和 RDP Session 启动 Host。
2. Windows 双机验证双向画面、鼠标、键盘、Unicode 文本剪贴板、文件剪贴板、连续多帧和断线重连。
3. Controller 错误只在客户端状态/UI 展示；Vue 永不创建远程画面窗口。
