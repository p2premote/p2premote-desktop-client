//! P2PRemote Tauri 应用入口
//! 对应原 Wails: client/internal/app/app.go

mod commands;
mod http;

/// 应用版本号
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

use tauri::Manager;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder},
    AppHandle, UserAttentionType,
};
use tracing::{debug, info, warn};

/// UI 日志目录：复用 core 的平台运行时日志目录。
pub(crate) fn app_log_dir() -> std::path::PathBuf {
    p2premote_core::config::machine_log_dir()
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[tauri::command]
fn flash_main_window(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    window
        .request_user_attention(Some(UserAttentionType::Informational))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn show_system_notification(title: String, body: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        // 使用 Shell_NotifyIconW 的气球通知（balloon toast），与原 PowerShell NotifyIcon 行为一致，
        // 但纯 Rust 调用 Windows API，无 spawn 进程开销，也无 Start-Sleep 阻塞。
        use std::ffi::OsStr;
        use std::iter;
        use std::os::windows::ffi::OsStrExt;
        use std::ptr;
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::UI::Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD,
            NOTIFYICONDATAW, NOTIFYICONDATAW_0,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, LoadIconW, IDI_APPLICATION, WS_EX_LAYERED,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
        };

        // 宽字符转换，零结尾
        fn to_wide(s: &str) -> Vec<u16> {
            OsStr::new(s).encode_wide().chain(iter::once(0)).collect()
        }

        // 截断到目标缓冲区容量（含结尾 \0）
        fn copy_wide_into(dst: &mut [u16], src: &[u16]) {
            let len = src.len().min(dst.len());
            dst[..len].copy_from_slice(&src[..len]);
            if len > 0 && len < dst.len() {
                dst[len] = 0;
            }
        }

        // 用系统预定义 "Static" 类创建一个隐藏的消息窗口作为托盘图标 owner，
        // 避免自行注册窗口类的样板代码。气球显示由系统接管，无需处理回调。
        let static_class = to_wide("Static");
        let window_name = to_wide("P2PRemoteNotify");
        let hwnd: HWND = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
                static_class.as_ptr(),
                window_name.as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err("failed to create notify owner window".to_string());
        }

        let title_w = to_wide(&title);
        let body_w = to_wide(&body);
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = 1;
        // 自定义回调消息 ID（这里不实际处理，但 NIF_MESSAGE 要求设置一个非 0 值）
        data.uCallbackMessage = 0x8000; // WM_APP
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_INFO;
        data.hIcon = unsafe { LoadIconW(ptr::null_mut(), IDI_APPLICATION) };
        data.dwInfoFlags = NIIF_INFO;
        copy_wide_into(&mut data.szInfo, &body_w);
        copy_wide_into(&mut data.szInfoTitle, &title_w);
        copy_wide_into(&mut data.szTip, &title_w);
        // 设置气球显示时长（位于 Anonymous 联合体中）
        data.Anonymous = NOTIFYICONDATAW_0 { uTimeout: 8000 };

        let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) };
        let _ = unsafe { DestroyWindow(hwnd) };
        if added == 0 {
            return Err("Shell_NotifyIconW failed".to_string());
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("notify-send")
            .arg(title)
            .arg(body)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("failed to show system notification: {}", e))
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (title, body);
        Err("system notification is not supported on this platform".to_string())
    }
}

// 导入所有命令
use commands::{
    auth::{
        fetch_user_profile, get_invite_info, get_saved_login, get_user_info, has_saved_token,
        is_logged_in, login, logout, register_no_verify, reset_password, save_login_settings,
        send_verification_code, try_auto_login,
    },
    config::{
        check_update, exit_application, get_settings, get_wgvpn_lan_access_config,
        save_wgvpn_lan_access_config, set_auto_start, set_locale,
    },
    device::{
        delete_device, generate_connect_code, get_device_list, mark_current_device_offline,
        register_current_device_auto, set_device_password, update_device_alias,
    },
    service::{
        acknowledge_device_identity_notification, check_required_client_files,
        ensure_background_service_session, get_service_status, listen_service_events,
        refresh_service_network_info, refresh_tunnel_status, start_service_active_tunnel,
        start_service_anonymous_active_tunnel, stop_active_tunnel_job, stop_service_active_tunnel,
        stop_service_tunnel, sync_service_runtime_config, test_tunnel_speed,
    },
};
use p2premote_core::p2p::cleanup_orphan_p2plink_processes;

/// 对齐 Go 格式的自定义文件 MakeWriter（每日轮转）
mod go_logger {
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Write};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    const MAX_ROLLED_LOG_FILES: usize = 3;

    struct LogFile {
        file: File,
        date: String,
        stem: String,
        parent: PathBuf,
    }

    impl LogFile {
        fn new(log_dir: &PathBuf, name: &str) -> io::Result<Self> {
            let date = today();
            fs::create_dir_all(log_dir)?;
            let stem = name.to_string();
            let path = log_dir.join(format!("{}.log", name));
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            Ok(Self {
                file,
                date,
                stem,
                parent: log_dir.clone(),
            })
        }

        fn write(&mut self, buf: &str) -> io::Result<usize> {
            let current = today();
            if current != self.date {
                // 日期变了，重新打开文件（实现每日轮转）
                self.date = current;
                let path = self.parent.join(format!("{}-{}.log", self.stem, self.date));
                self.file = OpenOptions::new().create(true).append(true).open(&path)?;
                self.cleanup_rolled_logs();
            }
            self.file.write(buf.as_bytes())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }

        fn cleanup_rolled_logs(&self) {
            let prefix = format!("{}-", self.stem);
            let suffix = ".log";
            let Ok(entries) = fs::read_dir(&self.parent) else {
                return;
            };

            let mut logs = entries
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let path = entry.path();
                    let file_name = path.file_name()?.to_string_lossy().to_string();
                    if file_name.starts_with(&prefix) && file_name.ends_with(suffix) {
                        Some((file_name, path))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            logs.sort_by(|a, b| b.0.cmp(&a.0));
            for (_, path) in logs.into_iter().skip(MAX_ROLLED_LOG_FILES) {
                let _ = fs::remove_file(path);
            }
        }
    }

    fn today() -> String {
        chrono::Local::now().format("%Y-%m-%d").to_string()
    }

    /// 自定义 MakeWriter：对齐 Go 日志格式
    pub struct GoLogWriter {
        inner: Arc<Mutex<LogFile>>,
    }

    /// 实际写入器，持有 Arc 引用
    pub struct GoLogLine {
        inner: Arc<Mutex<LogFile>>,
    }

    impl GoLogWriter {
        pub fn new(log_dir: PathBuf) -> io::Result<Self> {
            Ok(Self {
                inner: Arc::new(Mutex::new(LogFile::new(&log_dir, "p2premote")?)),
            })
        }
    }

    impl Write for GoLogLine {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let s = String::from_utf8_lossy(buf);
            self.inner.lock().unwrap().write(&s)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.lock().unwrap().flush()
        }
    }

    impl<'a> MakeWriter<'a> for GoLogWriter {
        type Writer = GoLogLine;

        fn make_writer(&'a self) -> Self::Writer {
            GoLogLine {
                inner: self.inner.clone(),
            }
        }
    }
}

/// 初始化日志系统（对齐 Go: logs/p2premote-YYYY-MM-DD.log）
fn init_logging() {
    use tracing_subscriber::fmt::time::ChronoLocal;
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    // 日志输出目录：由 core 统一按平台解析
    let log_dir = app_log_dir();
    let file_writer = go_logger::GoLogWriter::new(log_dir.clone()).ok();

    let file_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("p2premote_lib=debug,hyper=info,reqwest=info,tokio=info,info")
    });

    let file_layer = file_writer.map(|file_writer| {
        fmt::layer()
            .with_writer(file_writer)
            .with_ansi(false)
            .with_target(true)
            .with_thread_ids(false)
            .with_file(true)
            .with_line_number(true)
            .with_level(true)
            .with_timer(ChronoLocal::rfc_3339())
    });

    let console_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("p2premote_lib=debug,hyper=info,reqwest=info,tokio=info,info")
    });

    let console_layer = fmt::layer()
        .with_ansi(true)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(true)
        .with_line_number(true)
        .with_level(true)
        .with_timer(ChronoLocal::rfc_3339());

    let subscriber = tracing_subscriber::registry()
        .with(file_filter)
        .with(console_filter)
        .with(console_layer)
        .with(file_layer);

    tracing::subscriber::set_global_default(subscriber).expect("failed to set tracing subscriber");
}

/// 初始化运行时数据目录。
fn init_config_dir() {
    let data_dir = p2premote_core::config::install_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);
    debug!("[p2premote.config] Data directory: {:?}", data_dir);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    init_config_dir();

    // 清理启动时遗留的 p2plink 进程
    cleanup_orphan_p2plink_processes();

    debug!("[p2premote] configuration is managed by background service");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .invoke_handler(tauri::generate_handler![
            login,
            logout,
            register_no_verify,
            send_verification_code,
            reset_password,
            is_logged_in,
            has_saved_token,
            get_user_info,
            fetch_user_profile,
            get_invite_info,
            get_saved_login,
            save_login_settings,
            try_auto_login,
            get_device_list,
            register_current_device_auto,
            update_device_alias,
            delete_device,
            set_device_password,
            generate_connect_code,
            mark_current_device_offline,
            get_settings,
            get_wgvpn_lan_access_config,
            save_wgvpn_lan_access_config,
            set_auto_start,
            set_locale,
            exit_application,
            check_update,
            check_required_client_files,
            ensure_background_service_session,
            get_service_status,
            listen_service_events,
            acknowledge_device_identity_notification,
            sync_service_runtime_config,
            start_service_active_tunnel,
            start_service_anonymous_active_tunnel,
            stop_active_tunnel_job,
            stop_service_active_tunnel,
            stop_service_tunnel,
            test_tunnel_speed,
            refresh_service_network_info,
            refresh_tunnel_status,
            flash_main_window,
            show_system_notification,
        ])
        .setup(|app| {
            info!("[p2premote] Tauri app setup complete");

            #[cfg(windows)]
            if let Err(err) = commands::config::ensure_notifier_running(&app.handle()) {
                warn!("[p2premote] Failed to start notifier: {}", err);
            }

            let logout_item = MenuItem::with_id(app, "logout", "退出登录", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "完全关闭", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&logout_item, &quit_item])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("p2pRemote")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id() == "logout" {
                        if let Err(err) = tauri::async_runtime::block_on(logout()) {
                            warn!("[p2premote] Failed to log out from tray menu: {}", err);
                            return;
                        }
                        if let Err(err) = p2premote_core::service_control::restart_service() {
                            warn!(
                                "[p2premote] Failed to restart background service after tray logout: {}",
                                err
                            );
                            return;
                        }
                        if let Err(err) = tauri::async_runtime::block_on(
                            commands::service::ensure_background_service_session(app.clone()),
                        ) {
                            warn!(
                                "[p2premote] Background service IPC was not ready after tray logout: {}",
                                err
                            );
                            return;
                        }
                        show_main_window(app);
                        if let Some(main_window) = app.get_webview_window("main") {
                            let _ = main_window.eval("window.location.reload()");
                        }
                    } else if event.id() == "quit" {
                        if let Err(err) =
                            tauri::async_runtime::block_on(mark_current_device_offline())
                        {
                            warn!(
                                "[p2premote] Failed to mark current device offline on quit: {}",
                                err
                            );
                        }
                        commands::config::stop_notifier();
                        commands::service::cleanup_background_service_on_app_exit();
                        app.exit(0);
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            #[cfg(target_os = "windows")]
            {
                if let Some(main_window) = app.get_webview_window("main") {
                    let main_window_for_close = main_window.clone();
                    main_window.on_window_event(move |event| {
                        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                            api.prevent_close();
                            let _ = main_window_for_close.hide();
                        }
                    });
                }
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
