//! 配置命令模块
//! 对应原 Go: internal/config 和设置相关的命令

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tracing::{debug, info};

use crate::APP_VERSION;
use p2premote_core::control::Data;

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
    app.path_resolver()
        .resource_dir()
        .map(|dir| notifier_executable_in(&dir))
        .ok_or_else(|| "failed to resolve application resource directory".to_string())
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
        return Err(format!(
            "notifier executable missing: {}",
            executable.display()
        ));
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

/// HKCU Run 自启动项的（名称, 命令行）期望值：主程序 + notifier。
#[cfg(windows)]
fn expected_run_entries(app: &AppHandle) -> Result<[(&'static str, String); 2], String> {
    let exe_path = std::env::current_exe().map_err(|e| e.to_string())?;
    let notifier = notifier_executable(app)?;
    Ok([
        ("p2premote", format!("\"{}\"", exe_path.display())),
        (
            "p2premote-notifier",
            format!("\"{}\" --agent", notifier.display()),
        ),
    ])
}

#[cfg(windows)]
fn set_service_auto_start(app: &AppHandle, enabled: bool) -> Result<(), String> {
    if enabled {
        let service_exe = crate::commands::service::resolve_service_executable(app)?;
        use p2premote_core::service_control::runas_scm_elevated_once;
        runas_scm_elevated_once("setup", &service_exe).map_err(|e| e.to_string())
    } else {
        p2premote_core::service_control::runas_scm_elevated("disable", None)
            .map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "linux")]
fn linux_gui_autostart_path() -> Result<std::path::PathBuf, String> {
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".config"))
        })
        .ok_or_else(|| "cannot locate the current user's XDG config directory".to_string())?;
    Ok(config_dir.join("autostart").join("p2premote-gui.desktop"))
}

#[cfg(target_os = "linux")]
fn linux_gui_autostart_entry(executable: &std::path::Path) -> Result<String, String> {
    let path = executable
        .to_str()
        .ok_or_else(|| "GUI executable path is not valid UTF-8".to_string())?;
    if path.contains('\n') || path.contains('\r') {
        return Err("GUI executable path contains a newline".to_string());
    }
    let escaped = path
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
        .replace('%', "%%");
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=p2pRemote\nExec=\"{escaped}\"\nIcon=p2premote\nTerminal=false\nX-p2pRemote-Managed=true\n"
    ))
}

#[cfg(target_os = "linux")]
fn write_linux_gui_autostart(
    path: &std::path::Path,
    contents: Option<&[u8]>,
) -> Result<(), String> {
    match contents {
        Some(contents) => {
            std::fs::create_dir_all(path.parent().expect("autostart path has parent"))
                .map_err(|err| format!("failed to create GUI autostart directory: {err}"))?;
            std::fs::write(path, contents)
                .map_err(|err| format!("failed to write GUI autostart entry: {err}"))
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("failed to remove GUI autostart entry: {err}")),
        },
    }
}

async fn load_service_auto_start() -> Result<bool, String> {
    match crate::commands::service::send_command_responsive(Data::GetLoginPreferences).await {
        Ok(Data::CommandResponse {
            data: Some(data), ..
        }) => data
            .get("auto_start")
            .and_then(|value| value.as_bool())
            .ok_or_else(|| "service response missing boolean field: auto_start".to_string()),
        Ok(other) => Err(format!("unexpected service response: {other:?}")),
        Err(error) => Err(error),
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
/// Windows 下使用 HKCU Run，Linux 下使用 XDG autostart；服务启动类型由服务管理。
/// 1. 写/删注册表 `HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Run`（用户级自启）；
/// 2. 装启/停后台 service（Windows 服务或 Linux systemd）；
/// 3. IPC 写 machine config 持久化自启状态；
/// 4. 任一步骤失败则回滚前面已改动的注册表与服务状态。
#[tauri::command]
pub async fn set_auto_start(app: AppHandle, enabled: bool) -> Result<(), String> {
    debug!("Set auto start: {}", enabled);

    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};
        use winreg::RegKey;

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
        let read_previous = |name: &str| match run_key.get_value::<String, _>(name) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!(
                "failed to read startup registry value {name}: {error}"
            )),
        };
        let previous_value = read_previous(app_name)?;
        let previous_notifier_value = read_previous(notifier_name)?;
        let previous_auto_start = load_service_auto_start().await?;

        if enabled {
            let entries = expected_run_entries(&app)?;
            for (name, command) in &entries {
                run_key.set_value(name, command).map_err(|e| {
                    crate::commands::localized(
                        "errors.write_registry_failed",
                        &[("reason", &e.to_string())],
                    )
                })?;
            }
            info!("[config] 已注册开机自启: {}", entries[0].1);
        } else {
            for name in [app_name, notifier_name] {
                if let Err(error) = run_key.delete_value(name) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        return Err(format!(
                            "failed to remove startup registry value {name}: {error}"
                        ));
                    }
                }
            }
            info!("[config] 已移除开机自启");
        }
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

    #[cfg(target_os = "linux")]
    {
        let _ = app;
        let path = linux_gui_autostart_path()?;
        let previous = match std::fs::read(&path) {
            Ok(contents) => Some(contents),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(format!("failed to read GUI autostart entry: {err}")),
        };
        let entry = if enabled {
            let executable = std::env::current_exe()
                .map_err(|err| format!("failed to locate GUI executable: {err}"))?;
            Some(linux_gui_autostart_entry(&executable)?)
        } else {
            None
        };
        write_linux_gui_autostart(&path, entry.as_ref().map(String::as_bytes))?;
        if let Err(err) = save_service_auto_start(enabled).await {
            if let Err(rollback) = write_linux_gui_autostart(&path, previous.as_deref()) {
                return Err(format!(
                    "{err}; failed to restore GUI autostart entry: {rollback}"
                ));
            }
            return Err(err);
        }
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        set_macos_login_item(&app, enabled)?;
        return save_service_auto_start(enabled).await;
    }

    #[allow(unreachable_code)]
    Ok(())
}

#[cfg(target_os = "macos")]
fn set_macos_login_item(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let _ = app;
    use p2premote_core::macos_service_management::RegistrationStatus;

    match p2premote_core::macos_service_management::set_main_app_login_item(enabled)
        .map_err(|error| format!("failed to update macOS Login Item: {error}"))?
    {
        RegistrationStatus::Enabled if enabled => Ok(()),
        RegistrationStatus::NotRegistered | RegistrationStatus::NotFound if !enabled => Ok(()),
        RegistrationStatus::RequiresApproval => Err(
            "macOS Login Item is awaiting approval in System Settings > General > Login Items"
                .to_string(),
        ),
        status => Err(format!(
            "macOS Login Item update returned unexpected status: {}",
            status.as_str()
        )),
    }
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
    pub release_notes: String,
    pub error: Option<String>,
}

/// Tauri 命令：检查更新
#[tauri::command]
pub async fn check_update() -> Result<UpdateCheckResponse, String> {
    info!("[config] 检查更新...");
    match crate::commands::service::send_command_responsive(Data::CheckUpdate {
        current_version: APP_VERSION.to_string(),
    })
    .await
    {
        Ok(Data::CommandResponse {
            ok: true,
            data: Some(data),
            ..
        }) => serde_json::from_value(data).map_err(|error| error.to_string()),
        Ok(Data::CommandResponse { message, .. }) => Err(message),
        Ok(other) => Err(format!("unexpected service response: {other:?}")),
        Err(error) => Err(error),
    }
}
