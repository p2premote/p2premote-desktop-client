# Windows 双机自动回归

## 范围

控制机通过 Windows OpenSSH 执行真实 CLI/IPC 操作，不点击 GUI。Python 驱动只使用标准库，需要 Python 3.11 及以上版本。

默认套件使用一个测试账号，覆盖以下场景：

| 场景 | 验证内容 |
| --- | --- |
| a_to_b / b_to_a | 双向发起连接，两端角色及虚拟地址一致，审批已完成，健康状态正常 |
| reconnect | 连接、清理、再次连接，每次都验证实际流量 |
| passive_disconnect | 被动端断开后，两端会话和未完成任务均结束 |
| cancel_disconnect | 发起连接后立即取消或断开，允许建链与取消竞争，但不能留下会话 |
| service_restart | 服务进程代次变化，重新登录，清理旧会话，再次建链 |
| peer_service_stop | 对端服务确实停止，主动端检测到丢失；恢复服务后清理 |

数据面使用现有 `TestTunnelSpeed`，要求上传、下载均为有限正数，且两端收发计数每项至少增加 65536 字节。只显示 connected、仅有心跳流量、测速数字存在但计数不动，均不能通过。Windows 使用用户态 WireGuard，不假定系统存在可直接访问的 TUN 地址。

失败立即结束后续用例，并尝试清理。清理失败也使整轮失败。服务丢失允许进入既有 degraded 宽限状态，不要求提前删除仍在宽限期内的会话。

## 一次性准备

两台虚拟机需要满足以下条件：

- 是可重装、可重启服务的专用测试机。
- 已配置 Windows OpenSSH、控制机密钥登录，且登录会话具有管理员权限。
- 控制机 known_hosts 已保存并核对两台主机的 SSH 主机密钥。脚本使用严格校验，不自动接受新主机密钥。
- 两台机器可访问同一业务服务器，测试账号可注册至少两个独立设备。
- 已准备实际安装目录。示例中的 CLI 路径需要按安装位置核对。

控制机需具备现有 Windows 打包脚本需要的 Rust、Go、Node、NSIS 和依赖仓库。完整打包仍使用 `scripts/build-windows.ps1`。

复制 `config.example.json` 为 `config.local.json`，填写 VM 地址、SSH 用户和版本。配置中只有环境变量名，不允许写入密码、token 或 secret。`minimum_probe_bytes` 可配置为 1024 至 1048576，默认 65536。

当前两台测试机 `10.10.100.200` 和 `10.10.100.52` 位于同一局域网。2026-10-18，用户已手动恢复 `.200` 的路由。后续自动化回归使用现有网络配置，不配置 `temporary_gateway`，也不添加或删除临时路由。网络异常时记录失败，由管理员检查网络配置。

账号通过进程环境注入。例如在本地 PowerShell 中先准备凭据，再启动回归：

```powershell
$credential = Get-Credential
$env:P2PREMOTE_TEST_IDENTIFIER = $credential.UserName
$env:P2PREMOTE_TEST_PASSWORD = $credential.GetNetworkCredential().Password

./scripts/regression/invoke.ps1 -ConfigPath ./scripts/regression/config.local.json -Version 1.14.0
```

上述入口先跑驱动和桌面单元测试，然后构建当前修订的安装包、校验 SHA-256、上传并静默安装到两台 VM，再运行回归。安装包安装失败或远端服务版本不匹配时直接失败。安装只使用 NSIS 的 `/S`，不会自动确认重启请求。

SSH 用户密码不参与此流程；远程执行使用已配置的密钥。测试账号密码经 SSH 标准输入传给 CLI，不放入子进程命令行。报告会递归移除凭据字段，并替换已知测试凭据。报告不采集可能含令牌的原始服务日志。

## 测试已安装版本

不重装、不重新打包：

```powershell
./scripts/regression/invoke.ps1 -ConfigPath ./scripts/regression/config.local.json -UseInstalledBuild
```

此模式严格核对配置里的 `expected_service_version`，验证的是已安装版本，不代表当前未部署的源码。

也可以直接使用 Python：

```powershell
python scripts/regression/run.py --config scripts/regression/config.local.json --output build/windows-regression --cases a_to_b,b_to_a --repeat 3
```

直接部署指定包时，配置两端各自的 `installer_env` 和 `installer_sha256_env`，将本地安装包路径与 SHA-256 注入对应环境变量，再加 `--deploy`。上传前后分别验证哈希；远端暂存位置固定为 `C:/ProgramData/P2PRemoteRegression/staging`。

新旧服务组合可配置不同的预期版本及安装包。旧包没有 `automation` 子命令时，需要事先在该 VM 提供当前 CLI 和匹配 DLL 的独立驱动目录，并将 `cli_path` 指向它。驱动通过机器指纹认证现有命名管道，不要求与服务位于同一安装目录。实际旧版本还必须具备套件要求的状态、测速和字节计数接口；缺失时会失败，不把缺失字段当作空会话而通过。默认 CI 构建和部署两端相同的新版本，不声称已完成旧版本矩阵。

## CLI 机器接口

`p2premote-cli automation --timeout-secs 30` 从标准输入读取一个最大 64 KiB 的现有 IPC JSON 命令，输出一个 JSON 对象。业务失败时 `ok=false`，保留结构化错误码，并返回非零退出码。

```json
{"t":"Status"}
```

```json
{"t":"StartActiveTunnelJob","c":{"peer_device_id":42,"peer_device_uuid":"target-uuid","lan_cidrs":[]}}
```

此入口使用显式允许列表。设备删除、服务退出、握手伪造等命令不能通过该入口执行。服务启动、停止和重启由现有 `service` 子命令执行。状态请求不会自动启动服务，因此能检查服务停止场景。

## 每次提交后的回归

`.github/workflows/windows-regression.yml` 在 main 提交及手动触发时运行。启用前需要：

1. 注册带 `Windows`、`X64`、`p2premote-regression` 标签的自托管 runner。
2. 在 runner 仓库目录之外保存实际配置文件。
3. 设置仓库变量 `WINDOWS_REGRESSION_ENABLED=true`、`WINDOWS_REGRESSION_CONFIG`，可选设置 `WINDOWS_REGRESSION_VERSION`。
4. 在 `windows-regression` environment 配置测试账号的两个 secret。私有依赖仓库还需 `CROSS_REPO_TOKEN`。

工作流检出桌面、打洞库及 WG FFI，构建当前版本，部署后验证。依赖修订、安装包哈希及服务预期版本记录到 `build-manifest.json`，实际 CLI 哈希记录在事件中。

两台 VM 是共享资源，CI 全局串行运行且不自动取消正在执行的安装或清理。工作流不在不可信 PR 上启动自托管 VM 测试。首次启用需要人工配置 runner 和 secret；提交 workflow 文件本身不会自动完成这些配置。

## 报告和本地验证

每次运行产生：

- `results.json`：场景结果及脱敏的操作、状态记录。
- `junit.xml`：CI 测试报告。
- `summary.md`：结果摘要、观测到的业务错误码，以及因前置失败而未执行的场景。
- `build-manifest.json`：由 PowerShell 入口记录构建来源。
- `effective-config.json`：本轮实际版本及配置，不包含账号密码。

本地检查测试驱动，无需 VM：

```powershell
python -m unittest discover -s scripts/regression -p test_*.py -v
cargo test -p p2premote-cli
```

内存替身测试包含正常全套流程，以及错误版本、重复设备身份、缺失状态字段、测速无流量、计数冻结、清理失败、地址不一致、SSH 失败和凭据脱敏。替身测试通过不能代替真实双机测试报告。

Windows 本地测试还会编译一个最小 CLI 替身，用真实 PowerShell 5.1 执行 agent，验证 UTF-8 输入、业务失败和退出码一致性。agent 通过压缩引导脚本发送，测试同时检查 Windows OpenSSH 默认 shell 的命令长度限制。

跨账号审批、复杂 NAT、防火墙故障、安装后重启和 GUI 展示不属于初版默认套件。一个账号和固定网络拓扑不能证明这些场景已通过。
