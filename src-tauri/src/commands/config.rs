//! 配置命令模块
//! 对应原 Go: internal/config 和设置相关的命令

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tracing::{debug, info, warn};

use crate::APP_VERSION;
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

/// 开机自启对账（应用启动时调用）：HKCU Run 注册表项 + Windows 服务启动类型。
///
/// 重装/升级的卸载阶段会删除 HKCU Run 自启动项（Tauri NSIS 模板删主程序项、
/// installer.nsh 的 PREUNINSTALL 删 notifier 项），也可能让 SCM 里的服务缺失或
/// 丢失 AutoStart；而 auto_start 状态持久化在 data/config.json，默认跨安装保留
/// ——出现"开关显示开启、实际不自启"的漂移。安装器 POSTINSTALL 会恢复；这里
/// 兜底其余场景（安装器运行账户与桌面用户不一致、安装路径变化等）。
#[cfg(windows)]
pub async fn reconcile_auto_start_registry(app: &AppHandle) {
    // 服务未安装时读不到 machine config（data 目录仅 SYSTEM/Admins 可读，只能走
    // service IPC），对账无从谈起；做一次不弹 UAC 的直接安装尝试后退出。
    match p2premote_core::service_control::query_service_status() {
        Ok(status) if !status.installed => {
            warn!("[config] 服务未安装，尝试直接重装服务");
            try_setup_service_direct(app);
            return;
        }
        Err(err) => {
            warn!("[config] 自启对账跳过：查询服务状态失败：{}", err);
            return;
        }
        _ => {}
    }

    // service 可能尚未就绪（刚开机/刚装完），带重试查询，总计约 60s。
    let mut enabled = None;
    for _ in 0..12 {
        match crate::commands::service::send_command_responsive(Data::GetLoginPreferences).await {
            Ok(Data::CommandResponse {
                data: Some(data), ..
            }) => {
                enabled = data.get("auto_start").and_then(|value| value.as_bool());
                break;
            }
            _ => tokio::time::sleep(std::time::Duration::from_secs(5)).await,
        }
    }
    if enabled != Some(true) {
        debug!("[config] 自启对账跳过：auto_start 未开启或查询失败");
        return;
    }

    use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let run_key = match hkcu.open_subkey_with_flags(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
        KEY_SET_VALUE | KEY_QUERY_VALUE,
    ) {
        Ok(key) => key,
        Err(err) => {
            warn!("[config] 自启对账失败：无法打开 HKCU Run 键：{}", err);
            return;
        }
    };
    let entries = match expected_run_entries(app) {
        Ok(entries) => entries,
        Err(err) => {
            warn!("[config] 自启对账失败：{}", err);
            return;
        }
    };
    for (name, command) in &entries {
        let current: String = run_key.get_value(name).unwrap_or_default();
        if current != *command {
            match run_key.set_value(name, command) {
                Ok(()) => info!("[config] 已修复开机自启注册表项 {} -> {}", name, command),
                Err(err) => warn!("[config] 修复开机自启注册表项 {} 失败：{}", name, err),
            }
        }
    }

    // auto_start 开启但服务非 AutoStart（重装把启动类型冲掉等）→ 直接修复。
    // 仅尝试 direct 路径（不弹 UAC）：普通非提权进程通常无 SCM 权限，失败即记日志，
    // 主修复责任在安装器 POSTINSTALL。
    match p2premote_core::service_control::query_service_status() {
        Ok(status) if status.installed && !status.enabled => {
            info!("[config] 服务非 AutoStart 但 auto_start 已开启，尝试直接修复");
            try_setup_service_direct(app);
        }
        _ => {}
    }
}

/// 尝试 install + enable + start 服务（direct-only，不触发 UAC）。
#[cfg(windows)]
fn try_setup_service_direct(app: &AppHandle) {
    match crate::commands::service::resolve_service_executable(app) {
        Ok(service_exe) => match p2premote_core::service_control::try_setup_direct(&service_exe) {
            Ok(()) => info!("[config] 服务直接修复成功"),
            Err(err) => warn!("[config] 服务直接修复失败（非提权进程无 SCM 权限属预期）：{}", err),
        },
        Err(err) => warn!("[config] 服务直接修复失败：{}", err),
    }
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
        let previous_value = run_key.get_value::<String, _>(app_name).ok();
        let previous_notifier_value = run_key.get_value::<String, _>(notifier_name).ok();

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
    pub release_notes: String,
    pub error: Option<String>,
}

/// Tauri 命令：检查更新
#[tauri::command]
pub async fn check_update() -> Result<UpdateCheckResponse, String> {
    info!("[config] 检查更新...");
    let config = p2premote_core::config::load_machine_config().unwrap_or_default();
    match p2premote_core::update::fetch_version_policy(&config.server_url).await {
        Ok(data) => {
            let evaluation = p2premote_core::update::evaluate_version_policy(APP_VERSION, &data);
            info!(
                "[config] 版本检查完成: current={}, latest={}, min_supported={}, mode={}",
                APP_VERSION, data.latest_version, data.min_supported_version, evaluation.mode
            );
            Ok(UpdateCheckResponse {
                mode: evaluation.mode.to_string(),
                has_update: evaluation.has_update,
                force_update: evaluation.force_update,
                current: APP_VERSION.to_string(),
                latest: data.latest_version,
                min_supported: data.min_supported_version,
                release_notes: data.release_notes,
                error: None,
            })
        }
        Err(error) => {
            warn!("[config] 检查更新失败: {:?}", error);
            let message = error.localized_message(config.locale.as_deref());
            Ok(UpdateCheckResponse {
                mode: "none".to_string(),
                has_update: false,
                force_update: false,
                current: APP_VERSION.to_string(),
                latest: String::new(),
                min_supported: String::new(),
                release_notes: String::new(),
                error: Some(message),
            })
        }
    }
}
