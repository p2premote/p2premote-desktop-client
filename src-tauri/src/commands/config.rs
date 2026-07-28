//! 配置命令模块
//! 对应原 Go: internal/config 和设置相关的命令

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tracing::{debug, info, warn};

use crate::{http::PublicHttpClient, APP_VERSION};
use p2premote_core::control::Data;
use p2premote_core::service_control::disable_service;

/// 配置响应（包含版本号）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsResponse {
    pub auto_start: bool,
    pub remember_me: bool,
    pub auto_login: bool,
    pub version: String,
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn notifier_path_matches_installed_resource_layout() {
        let root = std::path::Path::new(r"C:\Program Files\p2pRemote");
        assert_eq!(
            notifier_executable_in(root),
            root.join("resources").join("p2premote-notifier.exe")
        );
    }
}

#[cfg(windows)]
pub fn notifier_executable(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .resource_dir()
        .map(|dir| notifier_executable_in(&dir))
        .map_err(|err| err.to_string())
}

#[cfg(windows)]
fn notifier_executable_in(resource_dir: &std::path::Path) -> std::path::PathBuf {
    resource_dir
        .join("resources")
        .join("p2premote-notifier.exe")
}

#[cfg(windows)]
pub fn ensure_notifier_running(app: &AppHandle) -> Result<(), String> {
    let executable = notifier_executable(app)?;
    if !executable.exists() {
        return Err(format!("notifier executable missing: {}", executable.display()));
    }
    std::process::Command::new(executable)
        .arg("--agent")
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("failed to start notifier: {}", err))
}

pub fn stop_notifier() {
    #[cfg(windows)]
    {
        use std::io::Write;
        if let Ok(mut stream) = std::net::TcpStream::connect_timeout(
            &"127.0.0.1:48086".parse().expect("valid notifier address"),
            std::time::Duration::from_millis(300),
        ) {
            let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(300)));
            let _ = stream.write_all(br#"{"command":"shutdown"}"#);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WgvpnLanAccessConfig {
    pub enabled: bool,
    pub cidrs: Vec<String>,
}

/// Tauri 命令：获取配置
///
/// remember_me / auto_login 已下沉到 machine config（受保护），通过 IPC 向 service 查询。
/// 其余 UI 偏好仍来自 user config。
#[tauri::command]
pub async fn get_settings() -> Result<SettingsResponse, String> {
    debug!("Get settings");

    // 异步部分：认证偏好与 autostart 从 service 查询（machine config 受保护）。
    let response = crate::commands::service::send_command_responsive(
        p2premote_core::control::Data::GetLoginPreferences,
    )
    .await?;
    let data = match response {
        Data::CommandResponse {
            ok: true,
            data: Some(data),
            ..
        } => data,
        Data::CommandResponse {
            ok: false, message, ..
        } => return Err(message),
        other => return Err(format!("unexpected service response: {:?}", other)),
    };
    let required_bool = |key: &str| {
        data.get(key)
            .and_then(|value| value.as_bool())
            .ok_or_else(|| format!("service response missing boolean field: {}", key))
    };
    let remember_me = required_bool("remember_me")?;
    let auto_login = required_bool("auto_login")?;
    let auto_start = required_bool("auto_start")?;

    Ok(SettingsResponse {
        auto_start,
        remember_me,
        auto_login,
        version: APP_VERSION.to_string(),
    })
}

fn set_service_auto_start(app: &AppHandle, enabled: bool) -> Result<(), String> {
    if enabled {
        let service_exe = crate::commands::service::resolve_service_executable(app)?;
        #[cfg(windows)]
        {
            use p2premote_core::service_control::runas_scm_elevated_once;
            let direct = p2premote_core::service_control::try_setup_direct(&service_exe);
            if direct.is_ok() {
                return Ok(());
            }
            runas_scm_elevated_once("setup", &service_exe).map_err(|e| e.to_string())
        }

        #[cfg(not(windows))]
        {
            p2premote_core::service_control::install_service(&service_exe)
                .and_then(|_| p2premote_core::service_control::enable_service())
                .and_then(|_| p2premote_core::service_control::start_service())
                .map_err(|e| e.to_string())
        }
    } else {
        let direct = disable_service();
        if direct.is_ok() {
            return Ok(());
        }
        #[cfg(windows)]
        {
            p2premote_core::service_control::runas_scm_elevated("disable", None)
                .map_err(|e| e.to_string())
        }
        #[cfg(not(windows))]
        {
            p2premote_core::service_control::disable_service().map_err(|e| e.to_string())
        }
    }
}

async fn load_service_auto_start() -> bool {
    match crate::commands::service::send_command_responsive(Data::GetLoginPreferences).await {
        Ok(Data::CommandResponse {
            data: Some(data), ..
        }) => data
            .get("auto_start")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        _ => false,
    }
}

async fn save_service_auto_start(enabled: bool) -> Result<(), String> {
    match crate::commands::service::send_command_responsive(Data::SetAutoStartConfig { enabled })
        .await
    {
        Ok(Data::CommandResponse { ok: true, .. }) => Ok(()),
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {:?}", other)),
        Err(err) => Err(err),
    }
}

/// Tauri 命令：设置开机自启动。
///
/// Windows 下完整职责（Linux 下仅服务部分）：
/// 1. 清理旧版 Startup .bat 残留；
/// 2. 写/删注册表 `HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Run`（用户级自启）；
/// 3. 装启/停后台 service（Windows 服务或 Linux systemd）；
/// 4. IPC 写 machine config 持久化自启状态；
/// 5. 任一步骤失败则回滚前面已改动的注册表与服务状态。
#[tauri::command]
pub async fn set_auto_start(app: AppHandle, enabled: bool) -> Result<(), String> {
    debug!("Set auto start: {}", enabled);

    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};
        use winreg::RegKey;

        // 清理旧版本的 Startup .bat 文件
        if let Ok(appdata) = std::env::var("APPDATA") {
            let old_bat = std::path::PathBuf::from(appdata)
                .join(r"Microsoft\Windows\Start Menu\Programs\Startup")
                .join("p2premote.bat");
            if old_bat.exists() {
                let _ = std::fs::remove_file(&old_bat);
                info!("[config] 已清理旧版 Startup .bat 文件");
            }
        }

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let run_key = hkcu
            .open_subkey_with_flags(
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
                KEY_SET_VALUE | KEY_QUERY_VALUE,
            )
            .map_err(|e| {
                crate::commands::localized(
                    "errors.open_registry_run_failed",
                    &[("reason", &e.to_string())],
                )
            })?;

        let app_name = "p2premote";
        let notifier_name = "p2premote-notifier";
        let previous_value = run_key.get_value::<String, _>(app_name).ok();
        let previous_notifier_value = run_key.get_value::<String, _>(notifier_name).ok();

        if enabled {
            let exe_path = std::env::current_exe().map_err(|e| e.to_string())?;
            let exe_str = format!("\"{}\"", exe_path.display());
            run_key.set_value(app_name, &exe_str).map_err(|e| {
                crate::commands::localized(
                    "errors.write_registry_failed",
                    &[("reason", &e.to_string())],
                )
            })?;
            let notifier = notifier_executable(&app)?;
            let notifier_command = format!("\"{}\" --agent", notifier.display());
            run_key.set_value(notifier_name, &notifier_command).map_err(|e| {
                crate::commands::localized("errors.write_registry_failed", &[("reason", &e.to_string())])
            })?;
            info!("[config] 已注册开机自启: {}", exe_str);
        } else {
            let _ = run_key.delete_value(app_name); // 不存在时不报错
            let _ = run_key.delete_value(notifier_name);
            info!("[config] 已移除开机自启");
        }
        let previous_auto_start = load_service_auto_start().await;
        if let Err(err) = set_service_auto_start(&app, enabled) {
            if let Some(previous_value) = previous_value {
                let _ = run_key.set_value(app_name, &previous_value);
            } else {
                let _ = run_key.delete_value(app_name);
            }
            if let Some(previous_value) = previous_notifier_value {
                let _ = run_key.set_value(notifier_name, &previous_value);
            } else {
                let _ = run_key.delete_value(notifier_name);
            }
            return Err(err);
        }

        if let Err(err) = save_service_auto_start(enabled).await {
            if let Some(previous_value) = previous_value {
                let _ = run_key.set_value(app_name, &previous_value);
            } else {
                let _ = run_key.delete_value(app_name);
            }
            if let Some(previous_value) = previous_notifier_value {
                let _ = run_key.set_value(notifier_name, &previous_value);
            } else {
                let _ = run_key.delete_value(notifier_name);
            }
            let _ = set_service_auto_start(&app, previous_auto_start);
            return Err(err.to_string());
        }

        return Ok(());
    }

    #[cfg(not(windows))]
    {
        set_service_auto_start(&app, enabled)?;
        return save_service_auto_start(enabled).await;
    }

    #[allow(unreachable_code)]
    Ok(())
}

#[tauri::command]
pub async fn get_wgvpn_lan_access_config() -> Result<WgvpnLanAccessConfig, String> {
    match crate::commands::service::send_command_responsive(Data::GetWgvpnLanAccessConfig).await {
        Ok(Data::CommandResponse {
            ok: true,
            data: Some(data),
            ..
        }) => serde_json::from_value::<WgvpnLanAccessConfig>(data).map_err(|e| e.to_string()),
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {:?}", other)),
        Err(err) => Err(err),
    }
}

#[tauri::command]
pub async fn save_wgvpn_lan_access_config(
    enabled: bool,
    cidrs: Vec<String>,
) -> Result<WgvpnLanAccessConfig, String> {
    match crate::commands::service::send_command_responsive(Data::SaveWgvpnLanAccessConfig {
        enabled,
        cidrs,
    })
    .await
    {
        Ok(Data::CommandResponse {
            ok: true,
            data: Some(data),
            ..
        }) => serde_json::from_value::<WgvpnLanAccessConfig>(data).map_err(|e| e.to_string()),
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {:?}", other)),
        Err(err) => Err(err),
    }
}

/// 设置 UI 语言并持久化到 service 侧 MachineConfig.locale。
/// 规范化在 service handler 内完成（只接受 zh-CN / en）。
#[tauri::command]
pub async fn set_locale(locale: String) -> Result<(), String> {
    match crate::commands::service::send_command_responsive(Data::SetLocale { locale }).await {
        Ok(Data::CommandResponse { ok: true, .. }) => Ok(()),
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {:?}", other)),
        Err(err) => Err(err),
    }
}

#[tauri::command]
pub fn exit_application(app_handle: tauri::AppHandle) {
    stop_notifier();
    crate::commands::service::cleanup_background_service_on_app_exit();
    app_handle.exit(0);
}

/// 版本检查响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResponse {
    pub mode: String,
    pub has_update: bool,
    pub force_update: bool,
    pub current: String,
    pub latest: String,
    pub min_supported: String,
    pub download_url: String,
    pub release_notes: String,
    pub error: Option<String>,
}

/// 服务器版本策略响应
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VersionPolicyResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<VersionPolicyData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VersionPolicyData {
    pub latest_version: String,
    pub min_supported_version: String,
    pub download_url: String,
    #[serde(default)]
    pub release_notes: String,
}

/// Tauri 命令：检查更新
#[tauri::command]
pub async fn check_update() -> Result<UpdateCheckResponse, String> {
    info!("[config] 检查更新...");

    let client = PublicHttpClient::new();
    let url = format!("{}{}", client.base_url(), "/api/v1/client/version-policy");

    let resp = client.client().get(&url).send().await;

    match resp {
        Ok(response) => {
            if response.status().is_success() {
                match response.json::<VersionPolicyResponse>().await {
                    Ok(policy_resp) => {
                        let data = match policy_resp.data {
                            Some(data) => data,
                            None => {
                                return Ok(UpdateCheckResponse {
                                    mode: "none".to_string(),
                                    has_update: false,
                                    force_update: false,
                                    current: APP_VERSION.to_string(),
                                    latest: String::new(),
                                    min_supported: String::new(),
                                    download_url: String::new(),
                                    release_notes: String::new(),
                                    error: Some(crate::commands::localized(
                                        "errors.version_policy_empty",
                                        &[],
                                    )),
                                });
                            }
                        };
                        let force_update =
                            is_version_less(APP_VERSION, &data.min_supported_version);
                        let has_update = is_version_less(APP_VERSION, &data.latest_version);
                        let mode = if force_update {
                            "force"
                        } else if has_update {
                            "optional"
                        } else {
                            "none"
                        };

                        info!(
                            "[config] 版本检查完成: current={}, latest={}, min_supported={}, mode={}",
                            APP_VERSION,
                            data.latest_version,
                            data.min_supported_version,
                            mode
                        );

                        Ok(UpdateCheckResponse {
                            mode: mode.to_string(),
                            has_update,
                            force_update,
                            current: APP_VERSION.to_string(),
                            latest: data.latest_version,
                            min_supported: data.min_supported_version,
                            download_url: data.download_url,
                            release_notes: data.release_notes,
                            error: None,
                        })
                    }
                    Err(e) => {
                        warn!("[config] 解析版本信息失败: {}", e);
                        Ok(UpdateCheckResponse {
                            mode: "none".to_string(),
                            has_update: false,
                            force_update: false,
                            current: APP_VERSION.to_string(),
                            latest: String::new(),
                            min_supported: String::new(),
                            download_url: String::new(),
                            release_notes: String::new(),
                            error: Some(crate::commands::localized(
                                "errors.parse_version_failed",
                                &[],
                            )),
                        })
                    }
                }
            } else {
                let status = response.status();
                warn!("[config] 服务器返回状态码: {}", status);
                Ok(UpdateCheckResponse {
                    mode: "none".to_string(),
                    has_update: false,
                    force_update: false,
                    current: APP_VERSION.to_string(),
                    latest: String::new(),
                    min_supported: String::new(),
                    download_url: String::new(),
                    release_notes: String::new(),
                    error: Some(crate::commands::localized(
                        "errors.server_status_code",
                        &[("status", &status.to_string())],
                    )),
                })
            }
        }
        Err(e) => {
            warn!("[config] 检查更新失败: {}", e);
            Ok(UpdateCheckResponse {
                mode: "none".to_string(),
                has_update: false,
                force_update: false,
                current: APP_VERSION.to_string(),
                latest: String::new(),
                min_supported: String::new(),
                download_url: String::new(),
                release_notes: String::new(),
                error: Some(crate::commands::localized(
                    "errors.cannot_connect_update_server",
                    &[],
                )),
            })
        }
    }
}

fn is_version_less(current: &str, target: &str) -> bool {
    let current_parts = parse_version_parts(current);
    let target_parts = parse_version_parts(target);
    let max_len = current_parts.len().max(target_parts.len());

    for index in 0..max_len {
        let current_part = *current_parts.get(index).unwrap_or(&0);
        let target_part = *target_parts.get(index).unwrap_or(&0);
        if current_part < target_part {
            return true;
        }
        if current_part > target_part {
            return false;
        }
    }

    false
}

fn parse_version_parts(version: &str) -> Vec<u32> {
    version
        .split('.')
        .map(|part| part.trim().parse::<u32>().unwrap_or(0))
        .collect()
}
