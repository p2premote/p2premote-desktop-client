use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SERVICE_NAME: &str = "p2premote-service";
pub const SERVICE_DISPLAY_NAME: &str = "p2pRemote Service";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub installed: bool,
    pub running: bool,
    pub enabled: bool,
    pub raw_state: String,
}

#[cfg(target_os = "linux")]
const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/p2premote-service.service";

#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::{anyhow, Context};
#[cfg(target_os = "linux")]
use std::path::PathBuf;

// ---- Windows 实现 ----

#[cfg(windows)]
mod win_impl {
    use super::{ServiceStatus, SERVICE_DISPLAY_NAME, SERVICE_NAME};
    use anyhow::{anyhow, Context, Result};
    use std::ffi::OsString;
    use std::path::Path;
    use std::thread;
    use std::time::{Duration, Instant};
    use tracing::{debug, error, info, warn};
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState,
        ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    const MGR_ACCESS: ServiceManagerAccess = ServiceManagerAccess::CONNECT;
    const SERVICE_STATE_TIMEOUT: Duration = Duration::from_secs(30);
    const SERVICE_STATE_POLL_INTERVAL: Duration = Duration::from_millis(200);

    fn connect_manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
        ServiceManager::local_computer(None::<&str>, access)
            .context("failed to connect to service control manager")
    }

    fn wait_for_service_state(target: ServiceState, timeout: Duration) -> Result<()> {
        let manager = connect_manager(MGR_ACCESS)?;
        let service = manager
            .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
            .context("failed to open service while waiting for state change")?;
        let deadline = Instant::now() + timeout;

        loop {
            let current = service
                .query_status()
                .context("failed to query service while waiting for state change")?
                .current_state;
            if current == target {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "timed out waiting for service state {:?}; current state is {:?}",
                    target,
                    current
                ));
            }
            thread::sleep(SERVICE_STATE_POLL_INTERVAL);
        }
    }

    /// Windows 错误码: ERROR_SERVICE_DOES_NOT_EXIST = 1060
    const ERROR_SERVICE_DOES_NOT_EXIST: u32 = 1060;

    /// 判断 windows_service::Error 是否为"服务不存在"
    fn is_service_not_found_raw(err: &windows_service::Error) -> bool {
        match err {
            windows_service::Error::Winapi(ref io) => {
                io.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32)
            }
            _ => false,
        }
    }

    fn service_requires_stop(state: ServiceState) -> bool {
        !matches!(state, ServiceState::Stopped | ServiceState::StopPending)
    }

    fn current_service_state() -> Result<Option<ServiceState>> {
        let manager = connect_manager(MGR_ACCESS)?;
        let service = match manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
            Ok(service) => service,
            Err(err) if is_service_not_found_raw(&err) => return Ok(None),
            Err(err) => return Err(err).context("failed to open service for state query"),
        };
        let status = service
            .query_status()
            .context("failed to query current service state")?;
        Ok(Some(status.current_state))
    }

    /// 通过 ShellExecuteW runas 提权执行 service exe 的 SCM 操作
    /// service exe 的 --scm 模式自行写退出码文件 + FreeConsole，无需 cmd.exe 包装
    pub fn runas_scm_elevated(action: &str, service_exe: Option<&Path>) -> Result<()> {
        let exe_path = match service_exe {
            Some(p) => p.to_path_buf(),
            None => find_service_executable()?,
        };

        if !exe_path.exists() {
            return Err(anyhow!(
                "service executable not found: {}",
                exe_path.display()
            ));
        }

        let launch_path = shell_compatible_path(&exe_path);
        let exe_str = launch_path
            .to_str()
            .ok_or_else(|| anyhow!("invalid path"))?;

        // 临时文件记录提权子进程退出码
        let tmp_dir = std::env::temp_dir();
        let exit_code_file = tmp_dir.join(format!(
            "p2premote-scm-{}-{}.exit",
            action,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&exit_code_file);

        // 构造参数：--scm <action> [exe_path] --exit-code-file <path>
        let mut params = if action == "install" || action == "setup" {
            format!("--scm {} \"{}\"", action, exe_str)
        } else {
            format!("--scm {}", action)
        };
        params.push_str(&format!(
            " --exit-code-file \"{}\"",
            exit_code_file.display()
        ));
        let log_dir = crate::config::machine_log_dir();
        params.push_str(&format!(" --log-dir \"{}\"", log_dir.display()));
        info!(
            action = action,
            service_exe = %launch_path.display(),
            original_service_exe = %exe_path.display(),
            exit_code_file = %exit_code_file.display(),
            "[service-control] launching elevated SCM helper"
        );
        debug!(
            action = action,
            params = %params,
            "[service-control] elevated SCM helper params"
        );

        // ShellExecuteW "runas" 直接启动 service exe，不经过 cmd
        let result = unsafe { shell_execute_runas(exe_str, &params) };

        if let Err(e) = result {
            let _ = std::fs::remove_file(&exit_code_file);
            error!(
                action = action,
                error = %e,
                "[service-control] failed to launch elevated SCM helper"
            );
            return Err(anyhow!("failed to execute elevated command: {}", e));
        }

        // 等待退出码文件出现（最多 60 秒）
        let timeout = std::time::Duration::from_secs(60);
        let start = std::time::Instant::now();
        loop {
            if let Ok(content) = std::fs::read_to_string(&exit_code_file) {
                let mut lines = content.lines();
                let code: i32 = lines.next().unwrap_or("").trim().parse().unwrap_or(-1);
                let detail = lines.collect::<Vec<_>>().join("\n");
                let _ = std::fs::remove_file(&exit_code_file);
                if code == 0 {
                    info!(
                        action = action,
                        "[service-control] elevated SCM helper completed successfully"
                    );
                    return Ok(());
                } else {
                    let suffix = if detail.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", detail.trim())
                    };
                    warn!(
                        action = action,
                        exit_code = code,
                        detail = %detail.trim(),
                        "[service-control] elevated SCM helper failed"
                    );
                    return Err(anyhow!(
                        "elevated {} failed with exit code {}{}",
                        action,
                        code,
                        suffix
                    ));
                }
            }
            if start.elapsed() > timeout {
                let _ = std::fs::remove_file(&exit_code_file);
                error!(
                    action = action,
                    service_exe = %launch_path.display(),
                    original_service_exe = %exe_path.display(),
                    exit_code_file = %exit_code_file.display(),
                    "[service-control] elevated SCM helper timed out before writing exit code"
                );
                return Err(anyhow!(
                    "elevated {} timed out (UAC may have been cancelled)",
                    action
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    fn shell_compatible_path(path: &Path) -> std::path::PathBuf {
        let raw = path.as_os_str().to_string_lossy();
        if let Some(stripped) = raw.strip_prefix("\\\\?\\UNC\\") {
            return std::path::PathBuf::from(format!("\\\\{}", stripped));
        }
        if let Some(stripped) = raw.strip_prefix("\\\\?\\") {
            return std::path::PathBuf::from(stripped);
        }
        path.to_path_buf()
    }

    /// ShellExecuteW "runas" 动词触发 UAC 提权
    /// SW_HIDE = 0 隐藏 console 窗口，UAC 弹窗由系统控制不受影响
    unsafe fn shell_execute_runas(file: &str, params: &str) -> Result<()> {
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "shell32")]
        extern "system" {
            fn ShellExecuteW(
                hwnd: *mut std::ffi::c_void,
                lpoperation: *const u16,
                lpfile: *const u16,
                lpparameters: *const u16,
                lpdirectory: *const u16,
                nshowcmd: i32,
            ) -> *mut std::ffi::c_void;
        }

        let verb: Vec<u16> = std::ffi::OsStr::new("runas")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let file_w: Vec<u16> = std::ffi::OsStr::new(file)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let params_w: Vec<u16> = std::ffi::OsStr::new(params)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // SW_HIDE = 0：隐藏 service exe 的控制台窗口。
        // UAC 弹窗由系统控制，不受 nShowCmd 影响，仍会正常显示。
        let result = ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file_w.as_ptr(),
            params_w.as_ptr(),
            std::ptr::null(),
            0,
        );

        if (result as isize) > 32 {
            Ok(())
        } else {
            Err(anyhow!("ShellExecuteW error {}", result as isize))
        }
    }

    /// 尝试在当前 exe 同目录找 p2premote-service.exe
    fn find_service_executable() -> Result<std::path::PathBuf> {
        let current_exe = std::env::current_exe().context("failed to get current exe path")?;
        let exe_dir = current_exe
            .parent()
            .ok_or_else(|| anyhow!("failed to get exe directory"))?;

        let candidates = [exe_dir.join("resources").join("p2premote-service.exe")];

        for candidate in &candidates {
            if candidate.exists() {
                return Ok(candidate.clone());
            }
        }

        Err(anyhow!(
            "p2premote-service.exe not found in {}",
            exe_dir.display()
        ))
    }

    fn make_service_info(exe_path: &Path, start_type: ServiceStartType) -> ServiceInfo {
        let launch_arguments = Some(crate::config::machine_log_dir())
            .map(|log_dir| vec![OsString::from("--log-dir"), log_dir.into_os_string()])
            .unwrap_or_default();

        ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe_path.to_path_buf(),
            launch_arguments,
            dependencies: vec![],
            account_name: None,
            account_password: None,
        }
    }

    fn sanitize_service_executable_path(path: &Path) -> std::path::PathBuf {
        let raw = path.to_string_lossy();
        let trimmed = raw.trim();

        let extracted = if let Some(rest) = trimmed.strip_prefix('"') {
            rest.split('"').next().unwrap_or(rest).trim()
        } else if let Some(index) = trimmed.to_ascii_lowercase().find(".exe") {
            &trimmed[..index + 4]
        } else {
            trimmed.split_whitespace().next().unwrap_or(trimmed)
        };

        std::path::PathBuf::from(extracted.trim())
    }

    fn service_info_from_existing(
        config: windows_service::service::ServiceConfig,
        start_type: ServiceStartType,
    ) -> ServiceInfo {
        ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY_NAME),
            service_type: config.service_type,
            start_type,
            error_control: config.error_control,
            executable_path: sanitize_service_executable_path(&config.executable_path),
            launch_arguments: Some(crate::config::machine_log_dir())
                .map(|log_dir| vec![OsString::from("--log-dir"), log_dir.into_os_string()])
                .unwrap_or_default(),
            dependencies: config.dependencies,
            account_name: config.account_name,
            account_password: None,
        }
    }

    pub fn install_service(executable_path: &Path) -> Result<()> {
        // 先尝试直接连接 SCM（如果已经有权限）
        let direct = try_connect_and_install(executable_path);
        if direct.is_ok() {
            return direct;
        }
        // 权限不足，通过 UAC 提权
        runas_scm_elevated("install", Some(executable_path))
    }

    pub fn try_connect_and_install(executable_path: &Path) -> Result<()> {
        let manager = connect_manager(MGR_ACCESS | ServiceManagerAccess::CREATE_SERVICE)?;

        let existing = manager.open_service(SERVICE_NAME, ServiceAccess::CHANGE_CONFIG);
        if let Ok(service) = existing {
            let info = make_service_info(executable_path, ServiceStartType::OnDemand);
            service.change_config(&info)?;
            return Ok(());
        }

        let info = make_service_info(executable_path, ServiceStartType::OnDemand);
        manager
            .create_service(&info, ServiceAccess::empty())
            .context("failed to create service")?;
        Ok(())
    }

    pub fn uninstall_service() -> Result<()> {
        if !super::query_service_status()?.installed {
            return Ok(());
        }
        let _ = stop_service();
        let direct = try_connect_and_uninstall();
        if direct.is_ok() {
            return direct;
        }
        runas_scm_elevated("uninstall", None)
    }

    pub fn try_connect_and_uninstall() -> Result<()> {
        let manager = connect_manager(MGR_ACCESS)?;
        let service = manager
            .open_service(SERVICE_NAME, ServiceAccess::DELETE)
            .context("failed to open service for deletion")?;
        service.delete().context("failed to delete service")?;
        Ok(())
    }

    pub fn start_service() -> Result<()> {
        let direct = try_connect_and_start();
        if direct.is_ok() {
            info!("[service-control] start_service: direct start succeeded");
            return direct;
        }
        info!("[service-control] start_service: direct failed, trying runas elevation");
        runas_scm_elevated("start", None)
    }

    pub fn try_connect_and_start() -> Result<()> {
        let manager = connect_manager(MGR_ACCESS)?;
        let _ = repair_service_config_if_needed();
        let service = manager
            .open_service(
                SERVICE_NAME,
                ServiceAccess::START | ServiceAccess::QUERY_STATUS,
            )
            .context("failed to open service for start")?;

        // 已在运行直接返回成功
        let status = service.query_status()?;
        if status.current_state == ServiceState::Running {
            info!("[service-control] service already running, skip start");
            return Ok(());
        }
        info!(
            current_state = ?status.current_state,
            "[service-control] calling StartServiceW..."
        );

        service
            .start(&[] as &[OsString])
            .map_err(|e| {
                warn!(error = ?e, "[service-control] StartServiceW failed");
                e
            })
            .context("failed to start service")?;
        info!("[service-control] StartServiceW returned Ok");
        Ok(())
    }

    pub fn repair_service_config_if_needed() -> Result<()> {
        let manager = connect_manager(MGR_ACCESS)?;
        let service = manager
            .open_service(
                SERVICE_NAME,
                ServiceAccess::QUERY_CONFIG | ServiceAccess::CHANGE_CONFIG,
            )
            .context("failed to open service for config repair")?;
        let config = service
            .query_config()
            .context("failed to query service config")?;
        let sanitized_path = sanitize_service_executable_path(&config.executable_path);
        info!(
            exe_path = %config.executable_path.display(),
            sanitized = %sanitized_path.display(),
            start_type = ?config.start_type,
            "[service-control] service config before repair"
        );
        if sanitized_path != config.executable_path {
            let start_type = config.start_type;
            let info = service_info_from_existing(config, start_type);
            service
                .change_config(&info)
                .context("failed to repair service config")?;
            info!(
                service_exe = %sanitized_path.display(),
                "[service-control] repaired service executable path"
            );
        }
        Ok(())
    }

    pub fn stop_service() -> Result<()> {
        let Some(state) = current_service_state()? else {
            return Ok(());
        };
        if !service_requires_stop(state) {
            return Ok(());
        }
        let direct = try_connect_and_stop();
        if direct.is_ok() {
            return direct;
        }
        runas_scm_elevated("stop", None)
    }

    pub fn try_connect_and_stop() -> Result<()> {
        let manager = connect_manager(MGR_ACCESS)?;
        let service = manager
            .open_service(SERVICE_NAME, ServiceAccess::STOP)
            .context("failed to open service for stop")?;
        service.stop().context("failed to stop service")?;
        Ok(())
    }

    pub fn restart_service() -> Result<()> {
        stop_service().context("failed to stop service before restart")?;
        wait_for_service_state(ServiceState::Stopped, SERVICE_STATE_TIMEOUT)
            .context("service did not stop before restart")?;
        start_service().context("failed to start service during restart")?;
        wait_for_service_state(ServiceState::Running, SERVICE_STATE_TIMEOUT)
            .context("service did not reach running state after restart")
    }

    pub fn enable_service() -> Result<()> {
        if !super::query_service_status()?.installed {
            return Err(anyhow!("service is not installed"));
        }
        let direct = try_connect_and_enable();
        if direct.is_ok() {
            return direct;
        }
        runas_scm_elevated("enable", None)
    }

    pub fn try_connect_and_enable() -> Result<()> {
        let manager = connect_manager(MGR_ACCESS)?;
        let service = manager
            .open_service(
                SERVICE_NAME,
                ServiceAccess::CHANGE_CONFIG | ServiceAccess::QUERY_CONFIG,
            )
            .context("failed to open service for config change")?;
        let config = service
            .query_config()
            .context("failed to query service config")?;
        let info = service_info_from_existing(config, ServiceStartType::AutoStart);
        service.change_config(&info)?;
        Ok(())
    }

    pub fn disable_service() -> Result<()> {
        if !super::query_service_status()?.installed {
            return Ok(());
        }
        let direct = try_connect_and_disable();
        if direct.is_ok() {
            return direct;
        }
        runas_scm_elevated("disable", None)
    }

    pub fn try_connect_and_disable() -> Result<()> {
        let manager = connect_manager(MGR_ACCESS)?;
        let service = manager
            .open_service(
                SERVICE_NAME,
                ServiceAccess::CHANGE_CONFIG | ServiceAccess::QUERY_CONFIG,
            )
            .context("failed to open service for config change")?;
        let config = service
            .query_config()
            .context("failed to query service config")?;
        let info = service_info_from_existing(config, ServiceStartType::OnDemand);
        service.change_config(&info)?;
        Ok(())
    }

    pub fn query_service_status() -> Result<ServiceStatus> {
        query_named_service_status(SERVICE_NAME)
    }

    pub fn query_named_service_status(service_name: &str) -> Result<ServiceStatus> {
        let not_installed = ServiceStatus {
            installed: false,
            running: false,
            enabled: false,
            raw_state: String::new(),
        };

        let manager = match connect_manager(MGR_ACCESS) {
            Ok(m) => m,
            Err(e) => {
                // SCM 连接失败 → 可能权限问题，不应伪装成"未安装"
                // 返回错误让调用方区分
                return Err(e);
            }
        };

        let service = manager.open_service(service_name, ServiceAccess::QUERY_STATUS);

        let service = match service {
            Ok(s) => s,
            Err(e) => {
                // 只有"服务不存在"才返回 not_installed，其他错误向上抛
                if is_service_not_found_raw(&e) {
                    return Ok(not_installed);
                }
                return Err(e).context("failed to open service for query");
            }
        };

        let enabled = manager
            .open_service(service_name, ServiceAccess::QUERY_CONFIG)
            .ok()
            .and_then(|service| service.query_config().ok())
            .map(|c| c.start_type == ServiceStartType::AutoStart)
            .unwrap_or(false);

        let status_result = service.query_status();
        let raw_state = match &status_result {
            Ok(s) => format!("{:?}", s.current_state),
            Err(e) => format!("query failed: {}", e),
        };
        let running = status_result
            .as_ref()
            .map(|s| s.current_state == ServiceState::Running)
            .unwrap_or(false);

        Ok(ServiceStatus {
            installed: true,
            running,
            enabled,
            raw_state,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn stop_decision_handles_pending_and_paused_states() {
            assert!(!service_requires_stop(ServiceState::Stopped));
            assert!(!service_requires_stop(ServiceState::StopPending));
            assert!(service_requires_stop(ServiceState::Running));
            assert!(service_requires_stop(ServiceState::Paused));
            assert!(service_requires_stop(ServiceState::PausePending));
        }
    }
}

#[cfg(windows)]
pub use win_impl::*;

/// direct-only SCM 操作：不尝试 runas 提权，失败直接返回错误。
/// 供 --scm 提权 helper 使用，避免递归提权。
#[cfg(windows)]
pub mod direct {
    pub use super::win_impl::query_service_status;
    pub use super::win_impl::{
        try_connect_and_disable as disable_service, try_connect_and_enable as enable_service,
        try_connect_and_install as install_service, try_connect_and_start as start_service,
        try_connect_and_stop as stop_service, try_connect_and_uninstall as uninstall_service,
    };
}

/// 直接尝试 install + enable + start（不提权），全部成功才返回 Ok
#[cfg(windows)]
pub fn try_setup_direct(service_exe: &Path) -> Result<()> {
    direct::install_service(service_exe)?;
    direct::enable_service()?;
    direct::start_service()?;
    Ok(())
}

/// 单次 UAC 提权执行复合操作（如 setup），传入 service exe 路径
#[cfg(windows)]
pub fn runas_scm_elevated_once(action: &str, service_exe: &Path) -> Result<()> {
    // 复用 win_impl 内的 runas_scm_elevated，传入 Some(service_exe)
    win_impl::runas_scm_elevated(action, Some(service_exe))
}

#[cfg(target_os = "linux")]
fn run_systemctl(args: &[&str]) -> Result<std::process::Output> {
    std::process::Command::new("systemctl")
        .args(args)
        .output()
        .with_context(|| format!("failed to run systemctl {}", args.join(" ")))
}

#[cfg(target_os = "linux")]
fn ensure_root() -> Result<()> {
    let output = std::process::Command::new("id")
        .arg("-u")
        .output()
        .context("failed to check effective uid")?;
    if String::from_utf8_lossy(&output.stdout).trim() == "0" {
        Ok(())
    } else {
        Err(anyhow!("Linux service management requires root privileges"))
    }
}

#[cfg(target_os = "linux")]
fn systemd_unit_path() -> &'static Path {
    Path::new(SYSTEMD_UNIT_PATH)
}

#[cfg(target_os = "linux")]
fn prefix_systemd_unit_path() -> PathBuf {
    crate::config::linux_install_root_dir()
        .join("systemd")
        .join("p2premote-service.service")
}

#[cfg(target_os = "linux")]
fn service_unit_contents(executable_path: &Path) -> String {
    let working_dir = crate::config::linux_install_root_dir();
    format!(
        "[Unit]\nDescription={display}\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart={exe} --foreground\nWorkingDirectory={working_dir}\nRestart=always\nRestartSec=3\n\n[Install]\nWantedBy=multi-user.target\n",
        display = SERVICE_DISPLAY_NAME,
        exe = executable_path.display(),
        working_dir = working_dir.display(),
    )
}

#[cfg(target_os = "linux")]
fn ensure_systemd_unit_dir() -> Result<()> {
    if let Some(parent) = systemd_unit_path().parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create systemd unit directory: {}",
                parent.display()
            )
        })?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_prefix_binaries(service_executable_path: &Path) -> Result<PathBuf> {
    let resources_dir = crate::config::linux_resources_dir();
    std::fs::create_dir_all(&resources_dir).with_context(|| {
        format!(
            "failed to create Linux resources directory: {}",
            resources_dir.display()
        )
    })?;

    let service_target = crate::config::default_service_path();
    copy_or_keep(
        service_executable_path,
        &service_target,
        "p2premote-service",
    )?;

    let source_dir = service_executable_path
        .parent()
        .ok_or_else(|| anyhow!("failed to resolve service executable directory"))?;
    for binary in ["p2premote-cli"] {
        let source = source_dir.join(binary);
        let target = resources_dir.join(binary);
        copy_or_keep(&source, &target, binary)?;
    }

    Ok(service_target)
}

#[cfg(target_os = "linux")]
fn copy_or_keep(source: &Path, target: &Path, display_name: &str) -> Result<()> {
    if source == target && target.exists() {
        return Ok(());
    }
    if source.exists() {
        std::fs::copy(source, target).with_context(|| {
            format!(
                "failed to install {} from {} to {}",
                display_name,
                source.display(),
                target.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755)).with_context(
                || format!("failed to chmod installed binary: {}", target.display()),
            )?;
        }
        return Ok(());
    }
    if target.exists() {
        return Ok(());
    }
    Err(anyhow!(
        "{} not found: source={}, target={}",
        display_name,
        source.display(),
        target.display()
    ))
}

#[cfg(target_os = "linux")]
pub fn install_service(executable_path: &Path) -> Result<()> {
    ensure_root()?;
    ensure_systemd_unit_dir()?;
    crate::config::ensure_machine_dirs()?;
    let installed_executable_path = install_prefix_binaries(executable_path)?;
    let unit_contents = service_unit_contents(&installed_executable_path);
    let prefix_unit = prefix_systemd_unit_path();
    if let Some(parent) = prefix_unit.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create prefix systemd directory: {}",
                parent.display()
            )
        })?;
    }
    std::fs::write(&prefix_unit, &unit_contents).with_context(|| {
        format!(
            "failed to write prefix systemd unit file: {}",
            prefix_unit.display()
        )
    })?;
    std::fs::write(systemd_unit_path(), unit_contents).with_context(|| {
        format!(
            "failed to write systemd unit file: {}",
            systemd_unit_path().display()
        )
    })?;
    let output = run_systemctl(&["daemon-reload"])?;
    if !output.status.success() {
        return Err(anyhow!(
            "systemctl daemon-reload failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn uninstall_service() -> Result<()> {
    ensure_root()?;
    let _ = stop_service();
    let _ = disable_service();
    if systemd_unit_path().exists() {
        std::fs::remove_file(systemd_unit_path()).with_context(|| {
            format!(
                "failed to remove systemd unit file: {}",
                systemd_unit_path().display()
            )
        })?;
    }
    let prefix_unit = prefix_systemd_unit_path();
    if prefix_unit.exists() {
        let _ = std::fs::remove_file(prefix_unit);
    }
    let output = run_systemctl(&["daemon-reload"])?;
    if !output.status.success() {
        return Err(anyhow!(
            "systemctl daemon-reload failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn start_service() -> Result<()> {
    ensure_root()?;
    let output = run_systemctl(&["start", SERVICE_NAME])?;
    if !output.status.success() {
        return Err(anyhow!(
            "systemctl start failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn stop_service() -> Result<()> {
    ensure_root()?;
    let output = run_systemctl(&["stop", SERVICE_NAME])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not loaded") || stderr.contains("not be found") {
            return Ok(());
        }
        return Err(anyhow!("systemctl stop failed: {}", stderr.trim()));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn restart_service() -> Result<()> {
    ensure_root()?;
    let output = run_systemctl(&["restart", SERVICE_NAME])?;
    if !output.status.success() {
        return Err(anyhow!(
            "systemctl restart failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn enable_service() -> Result<()> {
    ensure_root()?;
    let output = run_systemctl(&["enable", SERVICE_NAME])?;
    if !output.status.success() {
        return Err(anyhow!(
            "systemctl enable failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn disable_service() -> Result<()> {
    ensure_root()?;
    let output = run_systemctl(&["disable", SERVICE_NAME])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not loaded") || stderr.contains("No such file") {
            return Ok(());
        }
        return Err(anyhow!("systemctl disable failed: {}", stderr.trim()));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn query_service_status() -> Result<ServiceStatus> {
    if !systemd_unit_path().exists() {
        return Ok(ServiceStatus {
            installed: false,
            running: false,
            enabled: false,
            raw_state: String::new(),
        });
    }

    let active = run_systemctl(&["is-active", SERVICE_NAME])?;
    let enabled = run_systemctl(&["is-enabled", SERVICE_NAME])?;
    let raw_state = String::from_utf8_lossy(&active.stdout).trim().to_string();

    Ok(ServiceStatus {
        installed: true,
        running: active.status.success() && raw_state == "active",
        enabled: enabled.status.success(),
        raw_state,
    })
}

// ---- macOS launchd implementation ----

#[cfg(target_os = "macos")]
const MACOS_SERVICE_LABEL: &str = "top.p2premote.service";
#[cfg(target_os = "macos")]
const MACOS_PLIST_PATH: &str = "/Library/LaunchDaemons/top.p2premote.service.plist";

#[cfg(target_os = "macos")]
fn macos_bundled_plist_path() -> Option<std::path::PathBuf> {
    let executable = std::env::current_exe().ok()?;
    executable.ancestors().find_map(|path| {
        (path.file_name().and_then(|value| value.to_str()) == Some("Contents")).then(|| {
            path.join("Library")
                .join("LaunchDaemons")
                .join("top.p2premote.service.plist")
        })
    })
}

#[cfg(target_os = "macos")]
fn register_bundled_macos_daemon() -> Result<()> {
    use crate::macos_service_management::{set_daemon_registered, RegistrationStatus};

    match set_daemon_registered(true)? {
        RegistrationStatus::Enabled => Ok(()),
        RegistrationStatus::RequiresApproval => Err(anyhow!(
            "macOS background service is awaiting approval; enable p2pRemote in System Settings > General > Login Items"
        )),
        status => Err(anyhow!(
            "macOS background service registration did not become active: {}",
            status.as_str()
        )),
    }
}

#[cfg(target_os = "macos")]
fn shell_quote(value: &Path) -> String {
    format!("'{}'", value.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(target_os = "macos")]
fn run_macos_admin_script(script: &str) -> Result<()> {
    let is_root = std::process::Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
        .unwrap_or(false);
    let output = if is_root {
        std::process::Command::new("/bin/sh")
            .args(["-c", script])
            .output()
            .context("failed to execute privileged macOS service operation")?
    } else {
        let apple_script = format!(
            "do shell script \"{}\" with administrator privileges",
            script.replace('\\', "\\\\").replace('"', "\\\"")
        );
        std::process::Command::new("/usr/bin/osascript")
            .args(["-e", &apple_script])
            .output()
            .context("failed to request macOS administrator approval")?
    };
    if output.status.success() {
        return Ok(());
    }
    let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if error.contains("-128") || error.to_ascii_lowercase().contains("canceled") {
        return Err(anyhow!("macOS administrator approval was declined"));
    }
    Err(anyhow!("macOS service operation failed: {error}"))
}

#[cfg(target_os = "macos")]
fn macos_plist_contents(executable_path: &Path) -> String {
    let executable = executable_path.to_string_lossy();
    let working_dir = crate::config::macos_install_root_dir();
    let log_dir = crate::config::machine_log_dir();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{MACOS_SERVICE_LABEL}</string>
  <key>ProgramArguments</key><array><string>{executable}</string><string>--foreground</string></array>
  <key>GroupName</key><string>staff</string>
  <key>WorkingDirectory</key><string>{}</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{}/launchd.stdout.log</string>
  <key>StandardErrorPath</key><string>{}/launchd.stderr.log</string>
</dict></plist>
"#,
        working_dir.display(),
        log_dir.display(),
        log_dir.display(),
    )
}

#[cfg(target_os = "macos")]
pub fn install_service(executable_path: &Path) -> Result<()> {
    if !executable_path.exists() {
        return Err(anyhow!(
            "service executable not found: {}",
            executable_path.display()
        ));
    }
    if macos_bundled_plist_path()
        .as_ref()
        .is_some_and(|path| path.exists())
    {
        return register_bundled_macos_daemon();
    }

    // Development builds do not have an application bundle. Retain a
    // launchctl installer for local debugging; release bundles always take the
    // ServiceManagement path above.
    let resources_dir = crate::config::macos_resources_dir();
    let target = resources_dir.join(crate::config::default_service_binary_name());
    let install_root = crate::config::macos_install_root_dir();
    let run_dir = crate::config::platform_run_dir();
    let log_dir = crate::config::machine_log_dir();
    let temp_plist = std::env::temp_dir().join(format!(
        "top.p2premote.service-{}.plist",
        std::process::id()
    ));
    std::fs::write(&temp_plist, macos_plist_contents(&target))
        .context("failed to prepare macOS LaunchDaemon property list")?;
    let command = format!(
        "/usr/bin/install -d -m 755 {root} {resources} {run_dir} {log_dir} && /usr/bin/install -m 755 {source} {target} && /usr/bin/install -m 644 {temp_plist} {plist} && (/bin/launchctl bootout system/{label} >/dev/null 2>&1 || true) && /bin/launchctl bootstrap system {plist} && /bin/launchctl enable system/{label}",
        root = shell_quote(&install_root),
        resources = shell_quote(&resources_dir),
        run_dir = shell_quote(&run_dir),
        log_dir = shell_quote(&log_dir),
        source = shell_quote(executable_path),
        target = shell_quote(&target),
        temp_plist = shell_quote(&temp_plist),
        plist = shell_quote(Path::new(MACOS_PLIST_PATH)),
        label = MACOS_SERVICE_LABEL,
    );
    let result = run_macos_admin_script(&command);
    let _ = std::fs::remove_file(temp_plist);
    result
}

#[cfg(target_os = "macos")]
pub fn uninstall_service() -> Result<()> {
    if macos_bundled_plist_path()
        .as_ref()
        .is_some_and(|path| path.exists())
    {
        crate::macos_service_management::set_daemon_registered(false)?;
        return Ok(());
    }
    let command = format!(
        "/bin/launchctl bootout system/{label} >/dev/null 2>&1 || true; /bin/rm -f {plist}",
        label = MACOS_SERVICE_LABEL,
        plist = shell_quote(Path::new(MACOS_PLIST_PATH)),
    );
    run_macos_admin_script(&command)
}

#[cfg(target_os = "macos")]
pub fn start_service() -> Result<()> {
    let bundled = macos_bundled_plist_path()
        .as_ref()
        .is_some_and(|path| path.exists());
    if bundled {
        use crate::macos_service_management::RegistrationStatus;
        match crate::macos_service_management::daemon_status()? {
            RegistrationStatus::Enabled => {
                let running = std::process::Command::new("/bin/launchctl")
                    .args(["print", &format!("system/{MACOS_SERVICE_LABEL}")])
                    .output()
                    .map(|output| output.status.success())
                    .unwrap_or(false);
                if running {
                    return Ok(());
                }
                // A deliberate stop uses bootout while keeping the user's
                // approval. Re-register the bundled daemon to bootstrap it.
                crate::macos_service_management::set_daemon_registered(false)?;
                return register_bundled_macos_daemon();
            }
            RegistrationStatus::RequiresApproval => {
                return Err(anyhow!(
                    "macOS background service is awaiting approval; enable p2pRemote in System Settings > General > Login Items"
                ))
            }
            _ => return register_bundled_macos_daemon(),
        }
    }
    if !Path::new(MACOS_PLIST_PATH).exists() {
        return Err(anyhow!("macOS background service is not installed"));
    }
    let command = format!(
        "/bin/launchctl enable system/{label} && (/bin/launchctl bootstrap system {plist} >/dev/null 2>&1 || /bin/launchctl kickstart -k system/{label})",
        label = MACOS_SERVICE_LABEL,
        plist = shell_quote(Path::new(MACOS_PLIST_PATH)),
    );
    run_macos_admin_script(&command)
}

#[cfg(target_os = "macos")]
pub fn stop_service() -> Result<()> {
    run_macos_admin_script(&format!(
        "/bin/launchctl bootout system/{MACOS_SERVICE_LABEL} >/dev/null 2>&1 || true"
    ))
}

#[cfg(target_os = "macos")]
pub fn restart_service() -> Result<()> {
    run_macos_admin_script(&format!(
        "/bin/launchctl kickstart -k system/{MACOS_SERVICE_LABEL}"
    ))
}

#[cfg(target_os = "macos")]
pub fn enable_service() -> Result<()> {
    if macos_bundled_plist_path()
        .as_ref()
        .is_some_and(|path| path.exists())
    {
        return register_bundled_macos_daemon();
    }
    run_macos_admin_script(&format!(
        "/bin/launchctl enable system/{MACOS_SERVICE_LABEL}"
    ))
}

#[cfg(target_os = "macos")]
pub fn disable_service() -> Result<()> {
    if macos_bundled_plist_path()
        .as_ref()
        .is_some_and(|path| path.exists())
    {
        crate::macos_service_management::set_daemon_registered(false)?;
        return Ok(());
    }
    run_macos_admin_script(&format!(
        "/bin/launchctl disable system/{MACOS_SERVICE_LABEL}"
    ))
}

#[cfg(target_os = "macos")]
pub fn query_service_status() -> Result<ServiceStatus> {
    let bundled = macos_bundled_plist_path()
        .as_ref()
        .is_some_and(|path| path.exists());
    let registration = if bundled {
        Some(crate::macos_service_management::daemon_status()?)
    } else {
        None
    };
    let installed = bundled || Path::new(MACOS_PLIST_PATH).exists();
    if !installed {
        return Ok(ServiceStatus {
            installed: false,
            running: false,
            enabled: false,
            raw_state: "not-installed".to_string(),
        });
    }
    let status = std::process::Command::new("/bin/launchctl")
        .args(["print", &format!("system/{MACOS_SERVICE_LABEL}")])
        .output()
        .context("failed to query macOS background service")?;
    let raw = String::from_utf8_lossy(&status.stdout).to_string();
    let disabled = std::process::Command::new("/bin/launchctl")
        .args(["print-disabled", "system"])
        .output()
        .ok()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.contains(MACOS_SERVICE_LABEL) && line.contains("true"))
        })
        .unwrap_or(false);
    if let Some(registration) = registration {
        use crate::macos_service_management::RegistrationStatus;
        // A bundled CLI is not the main application process, so
        // SMAppService may report NotFound even while launchd is running the
        // daemon registered by its parent app. In that case launchd is the
        // authoritative runtime source and the embedded plist still proves
        // that this installation supports the service.
        let running = status.status.success();
        return Ok(ServiceStatus {
            installed,
            running,
            enabled: matches!(registration, RegistrationStatus::Enabled)
                || (matches!(registration, RegistrationStatus::NotFound) && running && !disabled),
            raw_state: if matches!(registration, RegistrationStatus::RequiresApproval) {
                "requires-approval".to_string()
            } else if running {
                raw.lines()
                    .find(|line| line.trim_start().starts_with("state ="))
                    .map(str::trim)
                    .unwrap_or("running")
                    .to_string()
            } else {
                registration.as_str().to_string()
            },
        });
    }
    Ok(ServiceStatus {
        installed,
        running: status.status.success(),
        enabled: !disabled,
        raw_state: if status.status.success() {
            raw.lines()
                .find(|line| line.trim_start().starts_with("state ="))
                .map(str::trim)
                .unwrap_or("running")
                .to_string()
        } else {
            "stopped".to_string()
        },
    })
}
