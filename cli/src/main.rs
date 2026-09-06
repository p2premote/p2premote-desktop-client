use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand};
use p2premote_core::config::{
    default_service_binary_name, default_service_path, load_machine_config, machine_config_path,
    machine_log_dir, save_machine_config,
};
use p2premote_core::control::{send_command, Data, RuntimeStatus};
use p2premote_core::invite::parse_invite_info;
use p2premote_core::service_control::{
    disable_service, enable_service, install_service, query_service_status, restart_service,
    start_service, stop_service, uninstall_service,
};
use p2premote_core::tunnel_view::tunnel_status_view;
use rpassword::prompt_password;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

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
    Desktop {
        #[command(subcommand)]
        command: DesktopCommands,
    },
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
    UpdateAlias {
        device_id: i64,
        alias: String,
    },
    Delete {
        device_id: i64,
    },
    SetPassword {
        device_id: i64,
        #[arg(long)]
        password: Option<String>,
    },
    GenerateConnectCode {
        device_id: i64,
    },
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

#[derive(Subcommand)]
enum DesktopCommands {
    Start { peer_device_id: i64 },
    Stop { peer_device_id: i64 },
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
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Login(args) => {
            let password = match args.password {
                Some(password) => password,
                None => prompt_password("password: ")?,
            };
            ensure_service_running().await?;
            let user = command_data_value(
                send_command(Data::Login {
                    identifier: args.identifier,
                    password,
                })
                .await?,
            )?;
            let device = command_data_value(send_command(Data::RegisterDevice).await?)?;
            println!(
                "logged in as {} ({})",
                user.get("username").and_then(Value::as_str).unwrap_or(""),
                user.get("email").and_then(Value::as_str).unwrap_or("")
            );
            println!(
                "device registered: {} ({})",
                device
                    .get("device_name")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                device.get("device_id").and_then(Value::as_i64).unwrap_or(0)
            );
        }
        Commands::Logout => {
            ensure_service_running().await?;
            if let Err(err) =
                command_data_value(send_command(Data::MarkCurrentDeviceOffline).await?)
            {
                eprintln!("warning: failed to mark current device offline: {err}");
            }
            command_data_value(send_command(Data::Logout).await?)?;
            stop_service()?;
            println!("logged out");
        }
        Commands::Status => {
            let cfg = load_machine_config()?;
            println!("config: {}", machine_config_path().display());
            println!("server_url: {}", cfg.server_url);
            let service_status = query_service_status()?;
            println!("service_status: {:?}", service_status);
            if service_status.running {
                let runtime = runtime_status().await?;
                println!(
                    "device_uuid: {}",
                    runtime
                        .get("device_uuid")
                        .and_then(Value::as_str)
                        .unwrap_or("not registered")
                );
                println!(
                    "logged_in: {}",
                    runtime
                        .get("logged_in")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                );
                println!("{}", serde_json::to_string_pretty(&runtime)?);
            } else {
                println!(
                    "device_uuid: {}",
                    cfg.device_uuid.as_deref().unwrap_or("not registered")
                );
                println!("logged_in: unavailable");
                println!("runtime: unavailable (service is not running)");
            }
        }
        Commands::Device {
            command: DeviceCommands::Register,
        } => {
            ensure_service_running().await?;
            let info = command_data_value(send_command(Data::RegisterDevice).await?)?;
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        Commands::Device {
            command: DeviceCommands::Info,
        } => {
            if query_service_status()?.running {
                let runtime = runtime_status().await?;
                println!(
                    "device_id: {:?}",
                    runtime.get("device_id").and_then(Value::as_i64)
                );
                println!(
                    "device_uuid: {}",
                    runtime
                        .get("device_uuid")
                        .and_then(Value::as_str)
                        .unwrap_or("not registered")
                );
            } else {
                let cfg = load_machine_config()?;
                println!("device_id: {:?}", cfg.device_id);
                println!(
                    "device_uuid: {}",
                    cfg.device_uuid.as_deref().unwrap_or("not registered")
                );
            }
        }
        Commands::Device {
            command: DeviceCommands::List,
        } => {
            print_command_data(send_service_command(Data::GetDeviceList).await?)?;
        }
        Commands::Device {
            command: DeviceCommands::UpdateAlias { device_id, alias },
        } => {
            print_command_response(
                send_service_command(Data::UpdateDeviceAlias { device_id, alias }).await?,
            )?;
        }
        Commands::Device {
            command: DeviceCommands::Delete { device_id },
        } => {
            print_command_response(send_service_command(Data::DeleteDevice { device_id }).await?)?;
        }
        Commands::Device {
            command:
                DeviceCommands::SetPassword {
                    device_id,
                    password,
                },
        } => {
            let password = match password {
                Some(password) => password,
                None => prompt_password("password: ")?,
            };
            print_command_response(
                send_service_command(Data::SetDevicePassword {
                    device_id,
                    password,
                })
                .await?,
            )?;
        }
        Commands::Device {
            command: DeviceCommands::GenerateConnectCode { device_id },
        } => {
            print_command_data(
                send_service_command(Data::GenerateConnectCode { device_id }).await?,
            )?;
        }
        Commands::Invite {
            command: InviteCommands::Info,
        } => {
            print_command_data(send_service_command(Data::GetInviteInfo).await?)?;
        }
        Commands::Tunnel {
            command: TunnelCommands::List,
        } => {
            let status = refreshed_tunnel_status().await?;
            let mut view = tunnel_status_view(&status);
            sanitize_cli_value(&mut view);
            println!("{}", serde_json::to_string_pretty(&view)?);
        }
        Commands::Tunnel {
            command: TunnelCommands::Stop(args),
        } => match (args.target_device_id, args.source_device_id) {
            (Some(target_device_id), None) => {
                print_command_response(
                    send_service_command(Data::StopActiveTunnel { target_device_id }).await?,
                )?;
            }
            (None, Some(source_device_id)) => {
                print_command_response(
                    send_service_command(Data::StopTunnel { source_device_id }).await?,
                )?;
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
                    send_service_command(Data::StartActiveTunnelJob {
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
                let parsed =
                    parse_invite_info(&invite).ok_or_else(|| anyhow!("invalid invite text"))?;
                print_command_response(
                    send_service_command(Data::StartAnonymousActiveTunnelJob {
                        connect_code: parsed.device_code,
                        temporary_password: parsed.temporary_password,
                    })
                    .await?,
                )?;
            }
            _ => return Err(anyhow!("use exactly one of --device-id or --invite")),
        },
        Commands::Desktop {
            command: DesktopCommands::Start { peer_device_id },
        } => {
            print_command_response(
                send_service_command(Data::StartDesktopSession { peer_device_id }).await?,
            )?;
        }
        Commands::Desktop {
            command: DesktopCommands::Stop { peer_device_id },
        } => {
            print_command_response(
                send_service_command(Data::StopDesktopSession { peer_device_id }).await?,
            )?;
        }
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
                send_service_command(Data::StopActiveTunnelJob { target_device_id }).await?,
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
                "p2p_punch_path" => println!("{}", cfg.p2p_punch_path),
                "log_level" => println!("{}", cfg.log_level),
                "web_admin_allowed_ip" => println!("{:?}", cfg.web_admin_allowed_ip),
                "web_admin_security_code" => println!("{:?}", cfg.web_admin_security_code),
                "web_admin_security_code_must_change" => {
                    println!("{}", cfg.web_admin_security_code_must_change)
                }
                "webui_enabled" => println!("{}", cfg.webui_enabled),
                other => return Err(anyhow!("unsupported config key: {}", other)),
            }
        }
        Commands::Config {
            command: ConfigCommands::Set { key, value },
        } => {
            let mut cfg = load_machine_config()?;
            let restart_for_webui_change = key == "webui_enabled";
            let service_was_running = if restart_for_webui_change {
                false
            } else {
                query_service_status()?.running
            };
            match key.as_str() {
                "server_url" => cfg.server_url = value,
                "log_level" => cfg.log_level = value,
                "p2p_punch_path" => cfg.p2p_punch_path = value,
                "web_admin_allowed_ip" => cfg.web_admin_allowed_ip = non_empty_config_value(value),
                "web_admin_security_code" => {
                    cfg.web_admin_security_code = non_empty_config_value(value);
                    cfg.web_admin_security_code_must_change = false;
                }
                "web_admin_security_code_must_change" => {
                    cfg.web_admin_security_code_must_change =
                        value.parse::<bool>().map_err(|_| {
                            anyhow!("web_admin_security_code_must_change must be true or false")
                        })?
                }
                "webui_enabled" => {
                    cfg.webui_enabled = value
                        .parse::<bool>()
                        .map_err(|_| anyhow!("webui_enabled must be true or false"))?
                }
                other => return Err(anyhow!("unsupported config key: {}", other)),
            }
            save_machine_config(&cfg)?;
            if restart_for_webui_change {
                restart_service()?;
                println!("config updated; service restarted");
            } else if service_was_running {
                let response = send_command(Data::ReloadConfig).await.map_err(|err| {
                    anyhow!("config saved, but running service reload failed: {err}")
                })?;
                command_data_value(response).map_err(|err| {
                    anyhow!("config saved, but running service rejected reload: {err}")
                })?;
                println!("config updated; running service reloaded");
            } else {
                println!("config updated; reload deferred because service is not running");
            }
        }
        Commands::Logs => {
            println!("{}", machine_log_dir().display());
        }
    }

    Ok(())
}

fn non_empty_config_value(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

async fn ensure_service_running() -> Result<()> {
    let status = query_service_status()?;
    if !status.installed {
        install_service(&default_service_executable()?)?;
    }
    if !status.running {
        start_service()?;
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if send_command(Data::Ping).await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "service IPC did not become ready within 10 seconds"
            ));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn send_service_command(data: Data) -> Result<Data> {
    ensure_service_running().await?;
    Ok(send_command(data).await?)
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
            let mut output = serde_json::json!({
                "ok": ok,
                "data": data,
                "status": status,
            });
            sanitize_cli_value(&mut output);
            output["message"] = Value::String(message);
            println!("{}", serde_json::to_string_pretty(&output)?);
            if ok {
                Ok(())
            } else {
                Err(anyhow!("service command failed"))
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
        Data::CommandResponse { ok, data, .. } => {
            if !ok {
                return Err(anyhow!("service command failed"));
            }
            let mut data = data.unwrap_or(Value::Null);
            sanitize_cli_value(&mut data);
            println!("{}", serde_json::to_string_pretty(&data)?);
            Ok(())
        }
        other => {
            println!("{}", serde_json::to_string_pretty(&other_to_json(other)?)?);
            Ok(())
        }
    }
}

fn command_data_value(resp: Data) -> Result<Value> {
    match resp {
        Data::CommandResponse { ok, data, .. } => {
            if ok {
                Ok(data.unwrap_or(Value::Null))
            } else {
                Err(anyhow!("service command failed"))
            }
        }
        other => Err(anyhow!(
            "unexpected service response: {:?}",
            other_to_json(other)?
        )),
    }
}

async fn runtime_status() -> Result<Value> {
    let status = command_runtime_status(Data::Status).await?;
    let mut value = serde_json::to_value(status).context("failed to serialize runtime status")?;
    sanitize_cli_value(&mut value);
    Ok(value)
}

async fn refreshed_tunnel_status() -> Result<RuntimeStatus> {
    command_runtime_status(Data::RefreshTunnelStatus).await
}

async fn command_runtime_status(command: Data) -> Result<RuntimeStatus> {
    match send_service_command(command).await? {
        Data::CommandResponse { ok, status, .. } => {
            if !ok {
                return Err(anyhow!("service command failed"));
            }
            status.ok_or_else(|| anyhow!("service response missing runtime status"))
        }
        other => Err(anyhow!(
            "unexpected service response: {:?}",
            other_to_json(other)?
        )),
    }
}

async fn find_device_uuid(target_device_id: i64) -> Result<String> {
    let devices = match send_service_command(Data::GetDeviceList).await? {
        Data::CommandResponse { ok, data, .. } => {
            if !ok {
                return Err(anyhow!("service command failed"));
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

fn other_to_json(data: Data) -> Result<Value> {
    serde_json::to_value(data).context("failed to serialize service response")
}

fn sanitize_cli_value(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for key in ["message", "last_error", "device_identity_message", "locale"] {
                object.remove(key);
            }
            for child in object.values_mut() {
                sanitize_cli_value(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                sanitize_cli_value(item);
            }
        }
        _ => {}
    }
}
