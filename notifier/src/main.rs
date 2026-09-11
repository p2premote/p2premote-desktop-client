#![cfg_attr(windows, windows_subsystem = "windows")]

use base64::Engine;
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::io::Read;

const NOTIFIER_ADDR: &str = "127.0.0.1:48086";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NotificationEvent {
    title: String,
    body: String,
    #[serde(default)]
    locale: String,
}

#[derive(Debug, Deserialize)]
struct AgentCommand {
    command: String,
}

fn is_english_locale(locale: &str, title: &str) -> bool {
    locale.trim().to_ascii_lowercase().starts_with("en")
        || (locale.trim().is_empty() && title.is_ascii())
}

fn estimated_body_lines(body: &str) -> i32 {
    let units = body.chars().fold(0usize, |total, ch| {
        total + if ch.is_ascii() { 1 } else { 2 }
    });
    ((units + 55) / 56).clamp(2, 3) as i32
}

#[cfg(windows)]
fn scale_for_dpi(value: i32, dpi: u32) -> i32 {
    ((value as i64 * dpi as i64 + 48) / 96) as i32
}

#[cfg(windows)]
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if let Some(encoded) = args
        .iter()
        .position(|arg| arg == "--show")
        .and_then(|i| args.get(i + 1))
    {
        if let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded) {
            if let Ok(event) = serde_json::from_slice::<NotificationEvent>(&bytes) {
                show_notification(event);
            }
        }
        return;
    }
    run_agent();
}

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn run_agent() {
    let listener = match std::net::TcpListener::bind(NOTIFIER_ADDR) {
        Ok(listener) => listener,
        Err(_) => return, // 端口已占用即视为 notifier 单实例已运行。
    };
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(_) => return,
    };
    let mut notification_processes: Vec<std::process::Child> = Vec::new();
    for connection in listener.incoming() {
        let Ok(stream) = connection else { continue };
        let mut buffer = Vec::with_capacity(1024);
        if stream.take(8192).read_to_end(&mut buffer).is_err() {
            continue;
        }
        if serde_json::from_slice::<AgentCommand>(&buffer)
            .map(|command| command.command == "shutdown")
            .unwrap_or(false)
        {
            for child in &mut notification_processes {
                let _ = child.kill();
            }
            return;
        }
        if serde_json::from_slice::<NotificationEvent>(&buffer).is_err() {
            continue;
        }
        notification_processes.retain_mut(|child| {
            child
                .try_wait()
                .map(|status| status.is_none())
                .unwrap_or(false)
        });
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&buffer);
        if let Ok(child) = std::process::Command::new(&executable)
            .args(["--show", &encoded])
            .spawn()
        {
            notification_processes.push(child);
        }
    }
}

#[cfg(windows)]
fn enable_best_dpi_awareness() {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    use windows_sys::Win32::UI::WindowsAndMessaging::SetProcessDPIAware;

    unsafe {
        let user32 = GetModuleHandleA(c"user32.dll".as_ptr().cast());
        if !user32.is_null() {
            if let Some(proc) = GetProcAddress(
                user32,
                c"SetProcessDpiAwarenessContext".as_ptr().cast(),
            ) {
                type SetProcessDpiAwarenessContextFn =
                    unsafe extern "system" fn(*mut core::ffi::c_void) -> i32;
                let set_process_dpi_awareness_context: SetProcessDpiAwarenessContextFn =
                    std::mem::transmute(proc);
                // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2. Resolve the API at
                // runtime because user32.dll on Windows 7 does not export it.
                if set_process_dpi_awareness_context(-4isize as *mut core::ffi::c_void) != 0 {
                    return;
                }
            }
        }
        // Available since Vista. Windows 7 uses system-aware scaling, while
        // newer Windows versions retain PMv2 through the branch above.
        SetProcessDPIAware();
    }
}

#[cfg(windows)]
fn window_dpi(hwnd: windows_sys::Win32::Foundation::HWND) -> u32 {
    use windows_sys::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, ReleaseDC, LOGPIXELSX};
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

    unsafe {
        let user32 = GetModuleHandleA(c"user32.dll".as_ptr().cast());
        if !user32.is_null() {
            if let Some(proc) = GetProcAddress(user32, c"GetDpiForWindow".as_ptr().cast()) {
                type GetDpiForWindowFn = unsafe extern "system" fn(
                    windows_sys::Win32::Foundation::HWND,
                ) -> u32;
                let get_dpi_for_window: GetDpiForWindowFn = std::mem::transmute(proc);
                return get_dpi_for_window(hwnd).max(96);
            }
        }

        let dc = GetDC(hwnd);
        if dc.is_null() {
            return 96;
        }
        let dpi = GetDeviceCaps(dc, LOGPIXELSX as i32);
        ReleaseDC(hwnd, dc);
        (dpi as u32).max(96)
    }
}

#[cfg(windows)]
fn show_notification(event: NotificationEvent) {
    use native_windows_gui as nwg;
    use std::sync::{Arc, Mutex};
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetWindowRect, SetWindowPos, HTCAPTION, HTTRANSPARENT, SWP_NOACTIVATE,
        SWP_NOZORDER, SWP_SHOWWINDOW, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_NCHITTEST,
        WS_EX_TOOLWINDOW,
    };

    // PMv2 on modern Windows; system-aware fallback on Windows 7.
    enable_best_dpi_awareness();
    if nwg::init().is_err() {
        return;
    }
    let body_lines = estimated_body_lines(&event.body);
    let width = 360;
    let height = 170 + (body_lines - 2) * 20;
    let english = is_english_locale(&event.locale, &event.title);

    let mut window = nwg::Window::default();
    let mut accent = nwg::Label::default();
    let mut brand = nwg::Label::default();
    let mut title = nwg::Label::default();
    let mut body = nwg::Label::default();
    let mut dismiss = nwg::Button::default();
    let mut close = nwg::Button::default();
    let mut timer = nwg::AnimationTimer::default();
    let mut brand_font = nwg::Font::default();
    let mut title_font = nwg::Font::default();
    let mut body_font = nwg::Font::default();
    let mut action_font = nwg::Font::default();
    let _ = nwg::Font::builder()
        .family("Microsoft YaHei UI")
        .size(12)
        .weight(600)
        .build(&mut brand_font);
    let _ = nwg::Font::builder()
        .family("Microsoft YaHei UI")
        .size(20)
        .weight(600)
        .build(&mut title_font);
    let _ = nwg::Font::builder()
        .family("Microsoft YaHei UI")
        .size(17)
        .build(&mut body_font);
    let _ = nwg::Font::builder()
        .family("Microsoft YaHei UI")
        .size(14)
        .weight(500)
        .build(&mut action_font);

    if nwg::Window::builder()
        .size((width, height))
        .position((0, 0))
        .title("p2pRemote")
        .flags(nwg::WindowFlags::POPUP)
        // 显示时由 SetWindowPos(SWP_NOACTIVATE) 保证不抢焦点；不要使用
        // WS_EX_NOACTIVATE，否则原生子按钮也无法可靠接收鼠标交互。
        .ex_flags(WS_EX_TOOLWINDOW)
        .topmost(true)
        .build(&mut window)
        .is_err()
    {
        return;
    }
    // 不要使用全尺寸背景 Label：Static 控件会拦截鼠标命中测试，
    // 导致上层按钮虽然可见却无法被 WindowFromPoint 命中、无法点击。
    // 客户区背景交由窗口类背景刷子（COLOR_BTNFACE）绘制，仍然跟随
    // 系统主题与高对比度配色。
    let _ = nwg::Label::builder()
        .text("")
        .position((0, 0))
        .size((4, height))
        .background_color(Some([19, 143, 122]))
        .parent(&window)
        .build(&mut accent);
    let _ = nwg::Label::builder()
        .text("p2pRemote")
        .position((20, 10))
        .size((310, 20))
        .font(Some(&brand_font))
        .parent(&window)
        .build(&mut brand);
    let _ = nwg::Label::builder()
        .text(&event.title)
        .position((20, 30))
        .size((290, 32))
        .font(Some(&title_font))
        .parent(&window)
        .build(&mut title);
    let body_height = body_lines * 22 + 4;
    let _ = nwg::Label::builder()
        .text(&event.body)
        .position((20, 66))
        .size((320, body_height))
        .font(Some(&body_font))
        .parent(&window)
        .build(&mut body);
    let _ = nwg::Button::builder()
        .text("×")
        .position((318, 10))
        .size((28, 28))
        .font(Some(&action_font))
        .focus(false)
        .parent(&window)
        .build(&mut dismiss);
    let action_text = if english {
        "View connection"
    } else {
        "查看连接"
    };
    let action_width = if english { 112 } else { 88 };
    let _ = nwg::Button::builder()
        .text(action_text)
        .position((width - action_width - 16, height - 42))
        .size((action_width, 30))
        .font(Some(&action_font))
        .focus(false)
        .parent(&window)
        .build(&mut close);
    let _ = nwg::AnimationTimer::builder()
        .interval(std::time::Duration::from_secs(1))
        .parent(&window)
        .build(&mut timer);

    let Some(hwnd) = window.handle.hwnd() else {
        return;
    };
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;
    let corner = DWMWCP_ROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const u32 as *const core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        );
    }
    let accent_hwnd = accent.handle.hwnd().unwrap() as windows_sys::Win32::Foundation::HWND;
    let brand_hwnd = brand.handle.hwnd().unwrap() as windows_sys::Win32::Foundation::HWND;
    let title_hwnd = title.handle.hwnd().unwrap() as windows_sys::Win32::Foundation::HWND;
    let body_hwnd = body.handle.hwnd().unwrap() as windows_sys::Win32::Foundation::HWND;
    let dismiss_hwnd = dismiss.handle.hwnd().unwrap() as windows_sys::Win32::Foundation::HWND;
    let close_hwnd = close.handle.hwnd().unwrap() as windows_sys::Win32::Foundation::HWND;

    let layout = move |dpi: u32| unsafe {
        let s = |value| scale_for_dpi(value, dpi);
        let window_width = s(width);
        let window_height = s(height);
        SetWindowPos(
            accent_hwnd,
            std::ptr::null_mut(),
            0,
            0,
            s(4),
            window_height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        SetWindowPos(
            brand_hwnd,
            std::ptr::null_mut(),
            s(20),
            s(10),
            s(280),
            s(20),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        SetWindowPos(
            title_hwnd,
            std::ptr::null_mut(),
            s(20),
            s(30),
            s(290),
            s(32),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        SetWindowPos(
            body_hwnd,
            std::ptr::null_mut(),
            s(20),
            s(66),
            s(320),
            s(body_lines * 22 + 4),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        SetWindowPos(
            dismiss_hwnd,
            std::ptr::null_mut(),
            s(318),
            s(10),
            s(28),
            s(28),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        SetWindowPos(
            close_hwnd,
            std::ptr::null_mut(),
            window_width - s(action_width + 16),
            window_height - s(44),
            s(action_width),
            s(30),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );

        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };
        GetMonitorInfoW(monitor, &mut monitor_info);
        let x = monitor_info.rcWork.right - window_width - s(16);
        let y = monitor_info.rcWork.bottom - window_height - s(16);
        SetWindowPos(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::HWND_TOPMOST,
            x,
            y,
            window_width,
            window_height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    };
    layout(window_dpi(hwnd));

    // Static/Label 子窗口默认吃掉命中测试。让文本区域穿透到父窗口，父窗口再
    // 返回 HTCAPTION，于是除按钮外的整张卡片都能像标题栏一样直接拖动。
    let mut drag_handlers = Vec::new();
    for (index, handle) in [&accent.handle, &brand.handle, &title.handle, &body.handle]
        .into_iter()
        .enumerate()
    {
        if let Ok(handler) =
            nwg::bind_raw_event_handler(handle, 0x20000 + index, move |_hwnd, msg, _w, _l| {
                (msg == WM_NCHITTEST).then_some(HTTRANSPARENT as isize)
            })
        {
            drag_handlers.push(handler);
        }
    }
    let raw_handler = nwg::bind_raw_event_handler(
        &window.handle,
        0x21000,
        move |_raw_hwnd, msg, w, _l| match msg {
            WM_NCHITTEST => Some(HTCAPTION as isize),
            WM_DPICHANGED => {
                layout(((w >> 16) as u32).max(96));
                Some(0)
            }
            WM_DISPLAYCHANGE => {
                layout(window_dpi(hwnd));
                Some(0)
            }
            _ => None,
        },
    )
    .ok();

    let elapsed = Arc::new(Mutex::new(0u32));
    let elapsed_for_handler = elapsed.clone();
    let close_handle = close.handle;
    let dismiss_handle = dismiss.handle;
    let timer_handle = timer.handle;
    let handler = nwg::full_bind_event_handler(&window.handle, move |event, _data, handle| {
        if event == nwg::Event::OnButtonClick && handle == close_handle {
            launch_main_app();
            nwg::stop_thread_dispatch();
        }
        if (event == nwg::Event::OnButtonClick && handle == dismiss_handle)
            || event == nwg::Event::OnWindowClose
        {
            nwg::stop_thread_dispatch();
        }
        if event == nwg::Event::OnTimerTick && handle == timer_handle {
            let mut pointer = POINT { x: 0, y: 0 };
            let mut bounds = RECT::default();
            unsafe {
                GetCursorPos(&mut pointer);
                GetWindowRect(hwnd, &mut bounds);
            }
            if pointer.x >= bounds.left
                && pointer.x <= bounds.right
                && pointer.y >= bounds.top
                && pointer.y <= bounds.bottom
            {
                return;
            }
            let mut seconds = elapsed_for_handler.lock().unwrap();
            *seconds += 1;
            if *seconds >= 20 {
                nwg::stop_thread_dispatch();
            }
        }
    });
    nwg::dispatch_thread_events();
    if let Some(handler) = raw_handler.as_ref() {
        let _ = nwg::unbind_raw_event_handler(handler);
    }
    for handler in &drag_handlers {
        let _ = nwg::unbind_raw_event_handler(handler);
    }
    nwg::unbind_event_handler(&handler);
}

#[cfg(windows)]
fn launch_main_app() {
    let Ok(notifier) = std::env::current_exe() else {
        return;
    };
    let Some(install_root) = notifier.parent().and_then(|resources| resources.parent()) else {
        return;
    };
    let executable = install_root.join("p2premote.exe");
    if executable.exists() {
        let _ = std::process::Command::new(executable).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_controls_action_language_without_title_guessing() {
        assert!(is_english_locale("en", "远程设备已连接"));
        assert!(!is_english_locale("zh-CN", "Remote device connected"));
        assert!(is_english_locale("", "Remote device connected"));
    }

    #[test]
    fn body_height_is_bounded_to_two_or_three_lines() {
        assert_eq!(estimated_body_lines("短消息"), 2);
        assert_eq!(estimated_body_lines(&"很长的设备名称".repeat(20)), 3);
    }

    #[test]
    fn older_payload_without_locale_remains_compatible() {
        let event: NotificationEvent =
            serde_json::from_str(r#"{"title":"远程设备已连接","body":"测试"}"#)
                .expect("legacy notification payload");
        assert!(event.locale.is_empty());
    }

    #[test]
    fn shutdown_command_is_distinct_from_notification_payload() {
        let command: AgentCommand =
            serde_json::from_str(r#"{"command":"shutdown"}"#).expect("shutdown command");
        assert_eq!(command.command, "shutdown");
        assert!(serde_json::from_str::<NotificationEvent>(r#"{"command":"shutdown"}"#).is_err());
    }
}
