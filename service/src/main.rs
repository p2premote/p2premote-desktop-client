use p2premote_core::config::{ensure_machine_dirs, load_machine_config, machine_log_dir};
use p2premote_core::logging::{register_log_level_reloader, DailyLogWriter};
#[cfg(windows)]
use p2premote_core::runtime::request_shutdown;
use p2premote_core::runtime::run_service_foreground;
#[cfg(windows)]
use p2premote_core::service_control::SERVICE_NAME;
#[cfg(windows)]
use tracing::error;
use tracing_subscriber::{prelude::*, reload, EnvFilter};

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use windows_service::define_windows_service;

#[cfg(windows)]
const SERVICE_TYPE: windows_service::service::ServiceType =
    windows_service::service::ServiceType::OWN_PROCESS;

#[cfg(windows)]
define_windows_service!(ffi_service_main, service_entry);

fn init_logging() {
    let _ = ensure_machine_dirs();
    let log_dir = machine_log_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    let configured_level = load_machine_config()
        .map(|config| config.log_level)
        .unwrap_or_else(|_| "info".to_string());
    let env_filter =
        service_env_filter(&configured_level).unwrap_or_else(|_| EnvFilter::new("info"));
    let (filter_layer, reload_handle) = reload::Layer::new(env_filter);
    let timer = tracing_subscriber::fmt::time::LocalTime::new(
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6][offset_hour sign:mandatory]:[offset_minute]"),
    );

    let initialized = if let Ok(file_writer) = DailyLogWriter::new(log_dir, "p2premote-service") {
        tracing_subscriber::registry()
            .with(filter_layer)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(file_writer)
                    .with_ansi(false)
                    .with_timer(timer),
            )
            .try_init()
    } else {
        tracing_subscriber::registry()
            .with(filter_layer)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_timer(timer),
            )
            .try_init()
    };

    if initialized.is_ok() {
        register_log_level_reloader(move |level| {
            let filter = service_env_filter(level)?;
            reload_handle
                .reload(filter)
                .map_err(|err| format!("failed to reload service log level: {}", err))
        });
    }
}

fn service_env_filter(configured_level: &str) -> Result<EnvFilter, String> {
    match std::env::var(EnvFilter::DEFAULT_ENV) {
        Ok(value) => {
            EnvFilter::try_new(value).map_err(|err| format!("invalid RUST_LOG filter: {}", err))
        }
        Err(_) => EnvFilter::try_new(configured_level)
            .map_err(|err| format!("invalid config log_level: {}", err)),
    }
}

fn apply_log_dir_arg() {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i + 1 < args.len() {
        if args[i] == "--log-dir" {
            std::env::set_var("P2PREMOTE_SERVICE_LOG_DIR", &args[i + 1]);
            return;
        }
        i += 1;
    }
}

fn apply_web_listen_arg() {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i + 1 < args.len() {
        if args[i] == "--web-listen" || args[i] == "--web-admin-listen" {
            std::env::set_var("P2PREMOTE_WEB_ADMIN_ADDR", &args[i + 1]);
            return;
        }
        i += 1;
    }
}

fn apply_runtime_args() {
    apply_log_dir_arg();
    apply_web_listen_arg();
}

/// SCM 命令行操作（需管理员权限，由 UI 通过 runas 调用）
/// 用法: p2premote-service.exe --scm <install|uninstall|start|stop|enable|disable> [exe_path] [--exit-code-file <path>]
/// 使用 direct-only 函数，避免在已提权进程内再次触发 UAC
/// 如果指定 --exit-code-file，操作完成后将退出码写入该文件
#[cfg(windows)]
fn handle_scm_command() -> anyhow::Result<()> {
    apply_runtime_args();
    scm_trace("entered --scm helper");
    // 释放控制台窗口，避免提权后闪黑窗
    #[link(name = "kernel32")]
    extern "system" {
        fn FreeConsole() -> i32;
    }
    unsafe {
        FreeConsole();
    }

    let args: Vec<String> = std::env::args().collect();
    scm_trace("service control command received (arguments redacted)");
    let action = args
        .get(2)
        .ok_or_else(|| anyhow::anyhow!("usage: p2premote-service.exe --scm <action> [exe_path]"))?;

    // 查找 --exit-code-file 参数
    let exit_code_file = {
        let mut path = None;
        let mut i = 3;
        while i < args.len() {
            if args[i] == "--exit-code-file" {
                path = args.get(i + 1).map(|s| std::path::PathBuf::from(s));
                break;
            }
            i += 1;
        }
        path
    };

    // install 的 exe_path 在 --exit-code-file 之前
    let install_exe = if action == "install" || action == "setup" {
        args.get(3)
            .filter(|s| !s.starts_with('-'))
            .map(|s| std::path::PathBuf::from(s))
    } else {
        None
    };

    let main_service_before = p2premote_core::service_control::query_service_status().ok();

    let mut result = match action.as_str() {
        "install" => install_exe
            .ok_or_else(|| anyhow::anyhow!("install requires exe_path argument"))
            .and_then(|exe_path| {
                run_scm_step("install", || {
                    p2premote_core::service_control::direct::install_service(&exe_path)
                })
            }),
        "uninstall" => run_scm_step("uninstall", || {
            // 先停并等停止再删除：对运行中的服务 DeleteService 只会"标记删除"，
            // 残留的标记状态会让紧随其后的 setup（重装场景）以
            // ERROR_SERVICE_MARKED_FOR_DELETE 失败，装完后服务缺失。
            if let Ok(status) = p2premote_core::service_control::direct::query_service_status() {
                if status.installed && status.running {
                    let _ = p2premote_core::service_control::direct::stop_service();
                    wait_until_service_stopped(std::time::Duration::from_secs(10));
                }
            }
            let result = p2premote_core::service_control::direct::uninstall_service();
            if result.is_ok() {
                wait_until_service_gone(std::time::Duration::from_secs(10));
            }
            result
        }),
        "start" => run_scm_step("start", || {
            p2premote_core::service_control::direct::start_service()
        }),
        "stop" => run_scm_step("stop", || {
            p2premote_core::service_control::direct::stop_service()
        }),
        "enable" => run_scm_step("enable", || {
            p2premote_core::service_control::direct::enable_service()
        }),
        "disable" => run_scm_step("disable", || {
            p2premote_core::service_control::direct::disable_service()
        }),
        // 复合操作：install + enable + start，一次 UAC 完成
        "setup" => install_exe
            .ok_or_else(|| anyhow::anyhow!("setup requires exe_path argument"))
            .and_then(|exe_path| {
                run_scm_step("setup.install", || {
                    p2premote_core::service_control::direct::install_service(&exe_path)
                })?;
                run_scm_step("setup.enable", || {
                    p2premote_core::service_control::direct::enable_service()
                })?;
                // 等待 SCM 完成 config 变更后再启动
                std::thread::sleep(std::time::Duration::from_millis(500));
                // 重试 start 最多 3 次
                let mut start_err = None;
                for attempt in 1..=3 {
                    match run_scm_step("setup.start", || {
                        p2premote_core::service_control::direct::start_service()
                    }) {
                        Ok(()) => {
                            start_err = None;
                            break;
                        }
                        Err(e) => {
                            scm_trace(&format!("setup.start attempt {} failed: {}", attempt, e));
                            start_err = Some(e);
                            if attempt < 3 {
                                std::thread::sleep(std::time::Duration::from_secs(1));
                            }
                        }
                    }
                }
                if let Some(e) = start_err {
                    scm_trace(&format!("setup.start failed after retries: {}", e));
                    return Err(e);
                }
                Ok(())
            }),
        other => Err(anyhow::anyhow!("unknown scm action: {}", other)),
    };

    if result.is_err() && matches!(action.as_str(), "setup" | "enable" | "disable") {
        let mut rollback_errors = Vec::new();
        if let Some(before) = main_service_before {
            let rollback = (|| -> anyhow::Result<()> {
                if !before.installed {
                    if let Ok(current) = p2premote_core::service_control::query_service_status() {
                        if current.running {
                            let _ = p2premote_core::service_control::direct::stop_service();
                        }
                        if current.installed {
                            p2premote_core::service_control::direct::uninstall_service()?;
                        }
                    }
                    return Ok(());
                }
                if before.enabled {
                    p2premote_core::service_control::direct::enable_service()?;
                } else {
                    p2premote_core::service_control::direct::disable_service()?;
                }
                if before.running {
                    p2premote_core::service_control::direct::start_service()?;
                } else {
                    let _ = p2premote_core::service_control::direct::stop_service();
                }
                Ok(())
            })();
            if let Err(error) = rollback {
                rollback_errors.push(format!("main service rollback failed: {error}"));
            }
        }
        if !rollback_errors.is_empty() {
            let original = result
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_else(|| "SCM transaction failed".to_string());
            result = Err(anyhow::anyhow!(
                "{original}; {}",
                rollback_errors.join("; ")
            ));
        }
    }

    // 将退出码写入临时文件（供 UI 进程读取）
    if let Some(ref path) = exit_code_file {
        let content = match &result {
            Ok(()) => "0".to_string(),
            Err(err) => format!("1\n{:#}", err),
        };
        let _ = std::fs::write(path, content);
    }
    scm_trace(&format!("result: {:?}", result));

    result
}

#[cfg(windows)]
fn run_scm_step<F>(name: &str, f: F) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<()>,
{
    scm_trace(&format!("step begin: {}", name));
    let result = f();
    scm_trace(&format!("step end: {} => {:?}", name, result));
    result
}

/// 轮询等待服务进入 Stopped（或已不存在）；超时不视为错误，交由后续步骤兜底。
#[cfg(windows)]
fn wait_until_service_stopped(timeout: std::time::Duration) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let stopped = p2premote_core::service_control::direct::query_service_status()
            .map(|s| !s.installed || s.raw_state == "Stopped")
            .unwrap_or(true);
        if stopped {
            return;
        }
        if std::time::Instant::now() >= deadline {
            scm_trace("wait_until_service_stopped timed out");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// 轮询等待 DeleteService 真正生效（查询返回未安装）。
#[cfg(windows)]
fn wait_until_service_gone(timeout: std::time::Duration) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let gone = p2premote_core::service_control::direct::query_service_status()
            .map(|s| !s.installed)
            .unwrap_or(true);
        if gone {
            return;
        }
        if std::time::Instant::now() >= deadline {
            scm_trace("wait_until_service_gone timed out");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

#[cfg(windows)]
fn scm_trace(message: &str) {
    use std::io::Write;

    let path =
        std::env::temp_dir().join(format!("p2premote-scm-helper-{}.log", std::process::id()));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{}", message);
    }
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|arg| arg == "--foreground") {
        apply_runtime_args();
        init_logging();
        tracing::info!(
            "[service] starting in foreground mode, pid={}",
            std::process::id()
        );
        return run_foreground();
    }

    if args.iter().any(|arg| arg == "--scm") {
        match handle_scm_command() {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("scm operation failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    apply_runtime_args();
    init_logging();

    match windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        Ok(()) => Ok(()),
        Err(err) => {
            error!("failed to start service dispatcher: {} ({:?})", err, err);
            run_foreground()
        }
    }
}

#[cfg(not(windows))]
fn main() -> anyhow::Result<()> {
    apply_runtime_args();
    init_logging();
    #[cfg(target_os = "linux")]
    {
        let backend = p2premote_core::wgvpn::initialize_linux_wireguard_backend();
        tracing::info!(
            "[service] Linux WireGuard backend selected: {}",
            backend.as_str()
        );
    }
    run_foreground()
}

fn run_foreground() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run_service_foreground())
}

#[cfg(windows)]
fn service_entry(_arguments: Vec<OsString>) {
    if let Err(err) = service_main() {
        error!("service main failed: {:#?}", err);
    }
}

#[cfg(windows)]
fn service_main() -> anyhow::Result<()> {
    use anyhow::Context;
    use std::sync::mpsc;
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};

    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

    let status_handle =
        service_control_handler::register(
            SERVICE_NAME,
            move |control_event| match control_event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    let _ = shutdown_tx.send(());
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            },
        ).context("register service control handler")?;

    // 报告 StartPending
    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::StartPending,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 1,
        wait_hint: std::time::Duration::from_secs(5),
        process_id: None,
    }).context("report StartPending")?;

    // 在独立线程运行 service 逻辑
    let service_result = std::thread::spawn(move || run_foreground());

    // 报告 Running
    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: std::time::Duration::from_secs(0),
        process_id: None,
    }).context("report Running")?;

    loop {
        match shutdown_rx.try_recv() {
            Ok(_) => break,
            Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if service_result.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // 报告 StopPending
    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::StopPending,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 1,
        wait_hint: std::time::Duration::from_secs(10),
        process_id: None,
    })?;

    // 通知 runtime 停止
    request_shutdown();

    // 等待 service 线程结束，并把 runtime 失败写入 service 日志。
    match service_result.join() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => error!("service runtime failed: {:#}", err),
        Err(_) => error!("service runtime panicked"),
    }

    // 报告 Stopped
    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: std::time::Duration::from_secs(0),
        process_id: None,
    })?;

    Ok(())
}
