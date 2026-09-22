# 独立 RustDeskTiny 集成边界

## 产品边界

RustDeskTiny 是独立安装、独立运行的产品，负责自己的 Windows Service、Host
监听、窗口、Session 迁移、崩溃恢复、升级和卸载。p2pRemote 不启动或停止远端
Host，也不通过 WGVPN 健康控制通道管理 RustDeskTiny 生命周期。

默认直连地址为：

```text
<对端 WGVPN 虚拟 IP>:21121
```

p2pRemote 在当前登录用户的 GUI 进程中启动本机 RustDeskTiny Controller 并传入该地址，
同时将地址复制到剪贴板；启动失败时用户可手动粘贴连接。连接认证完全由 RustDeskTiny
的临时密码或长期密码完成。

## 端口职责

- RustDeskTiny 默认监听 `0.0.0.0:21121`，监听配置由 RustDeskTiny 自己管理。
- p2pRemote 当前使用 `21121` 作为默认连接目标和界面提示。
- 后续允许用户为每台设备记录不同的 RustDeskTiny 端口。
- 设备级端口只是 p2pRemote 的连接目标/提示信息；保存或修改它不得写入、重启或
  重新配置 RustDeskTiny。

## 隧道控制协议

WGVPN `48082` 控制长连接只负责健康检查、心跳和测速。协议中不包含
`DesktopStart`、`DesktopReady`、`DesktopStop` 等桌面生命周期消息。

## 本地进程

p2pRemote 的 GUI 进程启动 Controller 时只执行：

```text
RustDeskTiny.exe --connect <对端虚拟 IP>:<设备记录端口或 21121>
```

启动后不保存进程句柄、不轮询状态，也不随隧道或 p2pRemote 退出而终止
RustDeskTiny。
