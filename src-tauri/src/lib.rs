//! P2PRemote Tauri 应用入口
//! 对应原 Wails: client/internal/app/app.go

mod commands;

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
    #[cfg(target_os = "macos")]
    {
        window.show().map_err(|error| error.to_string())?;
        window.unminimize().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
    }
    window
        .request_user_attention(Some(UserAttentionType::Informational))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn launch_windows_rdp(address: String) -> Result<(), String> {
    let address = address.trim();
    if address.is_empty() || address.chars().any(char::is_whitespace) {
        return Err("invalid Remote Desktop address".to_string());
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("mstsc.exe")
            .arg(format!("/v:{address}"))
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("failed to start Windows Remote Desktop: {error}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("Windows Remote Desktop is only available on Windows".to_string())
    }
}

// 导入所有命令
use commands::{
    auth::{
        fetch_user_profile, get_invite_info, get_saved_login, get_user_info, is_logged_in, login,
        logout, register_by_email_code, reset_password_by_email_code, resume_saved_session,
        save_login_settings, send_registration_verification_code,
        send_reset_password_verification_code, try_auto_login,
    },
    config::{
        check_update, exit_application, get_settings, get_wgvpn_lan_access_config,
        save_wgvpn_lan_access_config, set_auto_start, set_locale,
    },
    device::{
        delete_device, generate_connect_code, get_device_list, mark_current_device_offline,
        parse_invite_info, set_device_password, update_device_alias, wake_device,
    },
    service::{
        acknowledge_device_identity_notification, approve_inbound_tunnel,
        check_required_client_files, ensure_background_service_session, get_service_status,
        listen_service_events, refresh_service_network_info, refresh_tunnel_status,
        reject_inbound_tunnel, set_background_service_enabled, start_service_active_tunnel,
        start_service_anonymous_active_tunnel, start_service_desktop_session,
        stop_active_tunnel_job, stop_service_active_tunnel, stop_service_desktop_session,
        stop_service_tunnel, sync_service_runtime_config, test_tunnel_speed,
    },
};

/// 初始化日志系统（对齐 Go: logs/p2premote-YYYY-MM-DD.log）
fn init_logging() -> Result<(), String> {
    use tracing_subscriber::fmt::time::ChronoLocal;
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    // 日志输出目录：由 core 统一按平台解析
    let log_dir = app_log_dir();
    let file_writer = p2premote_core::logging::DailyLogWriter::new(log_dir, "p2premote")
        .map_err(|error| format!("failed to initialize application log file: {error}"))?;

    let file_filter = EnvFilter::new("p2premote_lib=debug,hyper=info,reqwest=info,tokio=info,info");

    let file_layer = fmt::layer()
        .with_writer(file_writer)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(true)
        .with_line_number(true)
        .with_level(true)
        .with_timer(ChronoLocal::rfc_3339());

    let console_filter =
        EnvFilter::new("p2premote_lib=debug,hyper=info,reqwest=info,tokio=info,info");

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

    tracing::subscriber::set_global_default(subscriber)
        .map_err(|error| format!("failed to set tracing subscriber: {error}"))
}

/// 初始化运行时数据目录。
fn init_config_dir() -> Result<(), String> {
    let data_dir = p2premote_core::config::install_data_dir();
    std::fs::create_dir_all(&data_dir).map_err(|error| {
        format!(
            "failed to create application data directory {}: {error}",
            data_dir.display()
        )
    })?;
    debug!("[p2premote.config] Data directory: {:?}", data_dir);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging().expect("application logging initialization failed");
    init_config_dir().expect("application data directory initialization failed");

    debug!("[p2premote] configuration is managed by background service");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .invoke_handler(tauri::generate_handler![
            login,
            logout,
            register_by_email_code,
            send_registration_verification_code,
            send_reset_password_verification_code,
            reset_password_by_email_code,
            is_logged_in,
            get_user_info,
            fetch_user_profile,
            get_invite_info,
            get_saved_login,
            save_login_settings,
            try_auto_login,
            resume_saved_session,
            get_device_list,
			wake_device,
            parse_invite_info,
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
            set_background_service_enabled,
            listen_service_events,
            acknowledge_device_identity_notification,
            sync_service_runtime_config,
            start_service_active_tunnel,
            start_service_anonymous_active_tunnel,
            start_service_desktop_session,
            stop_service_desktop_session,
            stop_active_tunnel_job,
            approve_inbound_tunnel,
            reject_inbound_tunnel,
            stop_service_active_tunnel,
            stop_service_tunnel,
            test_tunnel_speed,
            refresh_service_network_info,
            refresh_tunnel_status,
            flash_main_window,
            launch_windows_rdp,
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

            #[cfg(any(target_os = "windows", target_os = "macos"))]
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
