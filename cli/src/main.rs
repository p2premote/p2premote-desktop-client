use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand};
use p2premote_core::auth::login_and_persist;
use p2premote_core::config::{
    clear_machine_credentials, default_service_binary_name, default_service_path,
    load_machine_config, machine_config_path, machine_log_dir, save_machine_config,
};
use p2premote_core::control::{send_command, Data};
use p2premote_core::device::{get_current_device_uuid, register_current_device_auto};
use p2premote_core::service_control::{
    disable_service, enable_service, install_service, query_service_status, restart_service,
    start_service, stop_service, uninstall_service,
};
use rpassword::prompt_password;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "p2premote-cli")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Login(LoginArgs),
    Logout,
    Status,
    Device {
        #[command(subcommand)]
        command: DeviceCommands,
    },
    Invite {
        #[command(subcommand)]
        command: InviteCommands,
    },
    Tunnel {
        #[command(subcommand)]
        command: TunnelCommands,
    },
    Connect(ConnectArgs),
    Job {
        #[command(subcommand)]
        command: JobCommands,
    },
    Service {
        #[command(subcommand)]
        command: ServiceCommands,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    Logs,
    Wgvpn {
        #[command(subcommand)]
        command: WgvpnCommands,
    },
}

#[derive(Args)]
struct LoginArgs {
    #[arg(long)]
    identifier: String,
    #[arg(long)]
    password: Option<String>,
}

#[derive(Subcommand)]
enum DeviceCommands {
    Register,
    Info,
    List,
}

#[derive(Subcommand)]
enum InviteCommands {
    Info,
}

#[derive(Subcommand)]
enum TunnelCommands {
    List,
    Stop(TunnelStopArgs),
}

#[derive(Args)]
struct TunnelStopArgs {
    #[arg(long)]
    target_device_id: Option<i64>,
    #[arg(long)]
    source_device_id: Option<i64>,
}

#[derive(Args)]
struct ConnectArgs {
    #[arg(long)]
    device_id: Option<i64>,
    #[arg(long)]
    invite: Option<String>,
}

#[derive(Subcommand)]
enum JobCommands {
    List,
    Stop { target_device_id: i64 },
}

#[derive(Subcommand)]
enum ServiceCommands {
    Install,
    Uninstall,
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
    Status,
}

#[derive(Subcommand)]
enum ConfigCommands {
    Get { key: String },
    Set { key: String, value: String },
}
#[derive(Subcommand)]
enum WgvpnCommands {
    /// 主动端：连接目标设备建立 wgvpn 隧道
    Active {
        /// 目标设备 ID
        target_device_id: i64,
        /// 打洞口令（punch_token）
        token: String,
    },
    /// 被动端：等待被连接
    Passive {
        /// 发起方设备 ID
        source_device_id: i64,
        /// 打洞口令（punch_token）
        token: String,
        /// 显式开放给主动端访问的被动端 LAN 网段，可重复传入
        #[arg(long = "expose-lan-cidr")]
        expose_lan_cidrs: Vec<String>,
    },
    /// 停止指定会话
    Stop {
        /// 目标设备 ID
        target_device_id: i64,
    },
    /// 列出所有 wgvpn 会话与 job 状态
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Login(args) => {
            let password = match args.password {
                Some(password) => password,
                None => prompt_password("password: ")?,
            };
            let bundle = login_and_persist(&args.identifier, &password).await?;
            let mut cfg = load_machine_config()?;
            let device = register_current_device_auto(&mut cfg).await?;
            let _ = install_service(&default_service_executable()?);
            let _ = start_service();
            let _ = send_command(Data::UpdateAuth).await;
            let _ = send_command(Data::ReloadConfig).await;
            println!(
                "logged in as {} ({})",
                bundle.user.username, bundle.user.email
            );
            println!(
                "device registered: {} ({})",
                device.device_name, device.device_id
            );
        }
        Commands::Logout => {
            let _ = send_command(Data::Logout).await;
            let mut cfg = load_machine_config()?;
            // 登出彻底清除凭据（token + 记住会话标志），
            // 与 service 端 Data::Logout 行为一致。
            clear_machine_credentials(&mut cfg);
            save_machine_config(&cfg)?;
            println!("logged out");
        }
        Commands::Status => {
            let cfg = load_machine_config()?;
            println!("config: {}", machine_config_path().display());
            println!("server_url: {}", cfg.server_url);
            println!("device_uuid: {}", get_current_device_uuid(&cfg));
            println!("logged_in: {}", cfg.auth_token.is_some());
            println!("service_status: {:?}", query_service_status()?);
            if let Ok(resp) = send_command(Data::Status).await {
                println!("{}", serde_json::to_string_pretty(&resp)?);
            }
        }
        Commands::Device {
            command: DeviceCommands::Register,
        } => {
            let mut cfg = load_machine_config()?;
            let info = register_current_device_auto(&mut cfg).await?;
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        Commands::Device {
            command: DeviceCommands::Info,
        } => {
            let cfg = load_machine_config()?;
            println!("device_id: {:?}", cfg.device_id);
            println!("device_uuid: {}", get_current_device_uuid(&cfg));
        }
        Commands::Device {
            command: DeviceCommands::List,
        } => {
            print_command_data(send_command(Data::GetDeviceList).await?)?;
        }
        Commands::Invite {
            command: InviteCommands::Info,
        } => {
            print_command_data(send_command(Data::GetInviteInfo).await?)?;
        }
        Commands::Tunnel {
            command: TunnelCommands::List,
        } => {
            // 隧道实现已迁移到 WGVPN。RefreshTunnelStatus 会刷新生命周期和
            // 按需流量快照，再映射成 CLI 的 active_tunnels/passive_tunnels 输出分组。
            let status = refreshed_tunnel_status().await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&tunnel_list_from_status(&status))?
            );
        }
        Commands::Tunnel {
            command: TunnelCommands::Stop(args),
        } => match (args.target_device_id, args.source_device_id) {
            (Some(target_device_id), None) => {
                print_command_response(
                    send_command(Data::StopActiveTunnel { target_device_id }).await?,
                )?;
            }
            (None, Some(source_device_id)) => {
                print_command_response(send_command(Data::StopTunnel { source_device_id }).await?)?;
            }
            _ => {
                return Err(anyhow!(
                    "use exactly one of --target-device-id or --source-device-id"
                ));
            }
        },
        Commands::Connect(args) => match (args.device_id, args.invite) {
            (Some(target_device_id), None) => {
                let target_device_uuid = find_device_uuid(target_device_id).await?;
                print_command_response(
                    send_command(Data::StartActiveTunnelJob {
                        target_device_id,
                        target_device_uuid,
                        connect_code: None,
                        temporary_password: None,
                        lan_cidrs: Vec::new(),
                    })
                    .await?,
                )?;
            }
            (None, Some(invite)) => {
                let (connect_code, temporary_password) = parse_invite(&invite)?;
                print_command_response(
                    send_command(Data::StartAnonymousActiveTunnelJob {
                        connect_code,
                        temporary_password,
                    })
                    .await?,
                )?;
            }
            _ => return Err(anyhow!("use exactly one of --device-id or --invite")),
        },
        Commands::Job {
            command: JobCommands::List,
        } => {
            let status = runtime_status().await?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    status
                        .get("active_tunnel_jobs")
                        .unwrap_or(&Value::Array(vec![]))
                )?
            );
        }
        Commands::Job {
            command: JobCommands::Stop { target_device_id },
        } => {
            print_command_response(
                send_command(Data::StopActiveTunnelJob { target_device_id }).await?,
            )?;
        }
        Commands::Service {
            command: ServiceCommands::Install,
        } => {
            install_service(&default_service_executable()?)?;
            println!("service installed");
        }
        Commands::Service {
            command: ServiceCommands::Uninstall,
        } => {
            uninstall_service()?;
            println!("service uninstalled");
        }
        Commands::Service {
            command: ServiceCommands::Start,
        } => {
            start_service()?;
            println!("service started");
        }
        Commands::Service {
            command: ServiceCommands::Stop,
        } => {
            let _ = send_command(Data::ShutdownGracefully).await;
            stop_service()?;
            println!("service stopped");
        }
        Commands::Service {
            command: ServiceCommands::Restart,
        } => {
            let _ = send_command(Data::ShutdownGracefully).await;
            restart_service()?;
            println!("service restarted");
        }
        Commands::Service {
            command: ServiceCommands::Enable,
        } => {
            enable_service()?;
            println!("service enabled");
        }
        Commands::Service {
            command: ServiceCommands::Disable,
        } => {
            disable_service()?;
            println!("service disabled");
        }
        Commands::Service {
            command: ServiceCommands::Status,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&query_service_status()?)?
            );
        }
        Commands::Config {
            command: ConfigCommands::Get { key },
        } => {
            let cfg = load_machine_config()?;
            match key.as_str() {
                "server_url" => println!("{}", cfg.server_url),
                "device_uuid" => println!("{:?}", cfg.device_uuid),
                "device_id" => println!("{:?}", cfg.device_id),
                "wg_path" => println!("{}", cfg.wg_path),
                "wireguard_path" => println!("{}", cfg.wireguard_path),
                "p2p_punch_path" => println!("{}", cfg.p2p_punch_path),
                "log_level" => println!("{}", cfg.log_level),
                "web_admin_allowed_ip" => println!("{:?}", cfg.web_admin_allowed_ip),
                "web_admin_security_code" => println!("{:?}", cfg.web_admin_security_code),
                "webui_enabled" => println!("{}", cfg.webui_enabled),
                other => return Err(anyhow!("unsupported config key: {}", other)),
            }
        }
        Commands::Config {
            command: ConfigCommands::Set { key, value },
        } => {
            let mut cfg = load_machine_config()?;
            let restart_for_webui_change = key == "webui_enabled";
            match key.as_str() {
                "server_url" => cfg.server_url = value,
                "log_level" => cfg.log_level = value,
                "wg_path" => cfg.wg_path = value,
                "wireguard_path" => cfg.wireguard_path = value,
                "p2p_punch_path" => cfg.p2p_punch_path = value,
                "web_admin_allowed_ip" => cfg.web_admin_allowed_ip = non_empty_config_value(value),
                "web_admin_security_code" => {
                    cfg.web_admin_security_code = non_empty_config_value(value)
                }
                "webui_enabled" => {
                    cfg.webui_enabled = value.parse::<bool>().map_err(|_| {
                        anyhow!("webui_enabled must be true or false")
                    })?
                }
                other => return Err(anyhow!("unsupported config key: {}", other)),
            }
            save_machine_config(&cfg)?;
            if restart_for_webui_change {
                restart_service()?;
                println!("config updated; service restarted");
            } else {
                let _ = send_command(Data::ReloadConfig).await;
                println!("config updated");
            }
        }
        Commands::Logs => {
            println!("{}", machine_log_dir().display());
        }
        Commands::Wgvpn { command } => handle_wgvpn(command).await?,
    }

    Ok(())
}

fn non_empty_config_value(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

async fn handle_wgvpn(command: WgvpnCommands) -> Result<()> {
    match command {
        WgvpnCommands::Active {
            target_device_id,
            token,
        } => {
            print_command_response(
                send_command(Data::WgvpnStart {
                    peer_device_id: target_device_id,
                    token,
                    is_active: true,
                    lan_cidrs: Vec::new(),
                })
                .await?,
            )?;
        }
        WgvpnCommands::Passive {
            source_device_id,
            token,
            expose_lan_cidrs,
        } => {
            print_command_response(
                send_command(Data::WgvpnStart {
                    peer_device_id: source_device_id,
                    token,
                    is_active: false,
                    lan_cidrs: expose_lan_cidrs,
                })
                .await?,
            )?;
        }
        WgvpnCommands::Stop { target_device_id } => {
            print_command_response(
                send_command(Data::WgvpnStop {
                    peer_device_id: target_device_id,
                })
                .await?,
            )?;
        }
        WgvpnCommands::List => {
            print_command_data(send_command(Data::WgvpnList).await?)?;
        }
    }
    Ok(())
}

fn default_service_executable() -> Result<PathBuf> {
    let binary_name = default_service_binary_name();
    let current_exe = std::env::current_exe()?;
    let parent = current_exe
        .parent()
        .ok_or_else(|| anyhow!("failed to resolve cli directory"))?;

    let candidates = vec![
        parent.join(binary_name),
        parent.join("service").join(binary_name),
        parent.join("resources").join(binary_name),
        default_service_path(),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    Err(anyhow!("{} not found in {}", binary_name, parent.display()))
}

fn print_command_response(resp: Data) -> Result<()> {
    match resp {
        Data::CommandResponse {
            ok,
            message,
            data,
            status,
            ..
        } => {
            let output = serde_json::json!({
                "ok": ok,
                "message": message,
                "data": data,
                "status": status,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
            if ok {
                Ok(())
            } else {
                Err(anyhow!(message))
            }
        }
        other => {
            println!("{}", serde_json::to_string_pretty(&other_to_json(other)?)?);
            Ok(())
        }
    }
}

fn print_command_data(resp: Data) -> Result<()> {
    match resp {
        Data::CommandResponse {
            ok, message, data, ..
        } => {
            if !ok {
                return Err(anyhow!(message));
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&data.unwrap_or(Value::Null))?
            );
            Ok(())
        }
        other => {
            println!("{}", serde_json::to_string_pretty(&other_to_json(other)?)?);
            Ok(())
        }
    }
}

async fn runtime_status() -> Result<Value> {
    command_status(Data::Status).await
}

async fn refreshed_tunnel_status() -> Result<Value> {
    command_status(Data::RefreshTunnelStatus).await
}

async fn command_status(command: Data) -> Result<Value> {
    match send_command(command).await? {
        Data::CommandResponse {
            ok,
            message,
            status,
            ..
        } => {
            if !ok {
                return Err(anyhow!(message));
            }
            serde_json::to_value(status).context("failed to serialize runtime status")
        }
        other => Err(anyhow!(
            "unexpected service response: {:?}",
            other_to_json(other)?
        )),
    }
}

fn tunnel_list_from_status(status: &Value) -> Value {
    let sessions = status
        .get("wgvpn_sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut active_tunnels = Vec::new();
    let mut passive_tunnels = Vec::new();
    for mut session in sessions {
        let is_active = session
            .get("is_active")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some(object) = session.as_object_mut() {
            if let Some(peer_device_id) = object.get("peer_device_id").cloned() {
                let directional_key = if is_active {
                    "target_device_id"
                } else {
                    "source_device_id"
                };
                object.insert(directional_key.to_string(), peer_device_id);
            }
        }
        if is_active {
            active_tunnels.push(session);
        } else {
            passive_tunnels.push(session);
        }
    }

    serde_json::json!({
        "active_tunnels": active_tunnels,
        "passive_tunnels": passive_tunnels,
    })
}

async fn find_device_uuid(target_device_id: i64) -> Result<String> {
    let devices = match send_command(Data::GetDeviceList).await? {
        Data::CommandResponse {
            ok, message, data, ..
        } => {
            if !ok {
                return Err(anyhow!(message));
            }
            data.ok_or_else(|| anyhow!("device list response missing data"))?
        }
        other => {
            return Err(anyhow!(
                "unexpected service response: {:?}",
                other_to_json(other)?
            ))
        }
    };

    let list_value = devices.get("data").unwrap_or(&devices);
    let list = list_value
        .as_array()
        .ok_or_else(|| anyhow!("device list response data is not an array"))?;
    for device in list {
        if device.get("device_id").and_then(Value::as_i64) == Some(target_device_id) {
            return device
                .get("device_uuid")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .ok_or_else(|| anyhow!("target device missing device_uuid"));
        }
    }
    Err(anyhow!("target device not found: {}", target_device_id))
}

fn parse_invite(invite: &str) -> Result<(String, String)> {
    let mut connect_code = String::new();
    let mut temporary_password = String::new();

    for line in invite.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('：').or_else(|| trimmed.split_once(':')) {
            let normalized = key.trim();
            if normalized.contains("设备代码") || normalized.eq_ignore_ascii_case("code") {
                connect_code = value.trim().to_string();
                continue;
            }
            if normalized.contains("临时密码") || normalized.eq_ignore_ascii_case("password") {
                temporary_password = value.trim().to_string();
                continue;
            }
        }
    }

    if connect_code.is_empty() || temporary_password.is_empty() {
        let parts: Vec<&str> = invite.split_whitespace().collect();
        if parts.len() >= 2 {
            connect_code = parts[0].to_string();
            temporary_password = parts[1].to_string();
        }
    }

    if connect_code.is_empty() || temporary_password.is_empty() {
        return Err(anyhow!("invalid invite text"));
    }
    Ok((connect_code, temporary_password))
}

fn other_to_json(data: Data) -> Result<Value> {
    serde_json::to_value(data).context("failed to serialize service response")
}

#[cfg(test)]
mod tests {
    use super::tunnel_list_from_status;
    use serde_json::json;

    #[test]
    fn tunnel_list_uses_wgvpn_sessions_and_preserves_transfer_stats() {
        let output = tunnel_list_from_status(&json!({
            "active_tunnels": [{"legacy": true}],
            "passive_tunnels": [{"legacy": true}],
            "wgvpn_sessions": [
                {
                    "peer_device_id": 28,
                    "is_active": true,
                    "virtual_ip": "100.99.71.2",
                    "peer_virtual_ip": "100.99.71.41",
                    "received_bytes": 1024,
                    "transmitted_bytes": 2048
                },
                {
                    "peer_device_id": 29,
                    "is_active": false,
                    "virtual_ip": "100.99.71.41",
                    "peer_virtual_ip": "100.99.71.2",
                    "received_bytes": 4096,
                    "transmitted_bytes": 8192
                }
            ]
        }));

        let active = output["active_tunnels"].as_array().unwrap();
        let passive = output["passive_tunnels"].as_array().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(passive.len(), 1);
        assert_eq!(active[0]["target_device_id"], 28);
        assert_eq!(active[0]["received_bytes"], 1024);
        assert_eq!(active[0]["transmitted_bytes"], 2048);
        assert_eq!(passive[0]["source_device_id"], 29);
        assert_eq!(passive[0]["received_bytes"], 4096);
        assert_eq!(passive[0]["transmitted_bytes"], 8192);
        assert!(active[0].get("legacy").is_none());
    }

    #[test]
    fn tunnel_list_defaults_to_empty_when_wgvpn_status_is_missing() {
        let output = tunnel_list_from_status(&json!({}));
        assert_eq!(output, json!({"active_tunnels": [], "passive_tunnels": []}));
    }
}
