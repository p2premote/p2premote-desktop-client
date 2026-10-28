use image::{imageops::FilterType, RgbaImage};
use tauri::{AppHandle, Manager};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::{
        LibraryLoader::{GetModuleHandleW, GetProcAddress},
        RemoteDesktop::{
            WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
            NOTIFY_FOR_THIS_SESSION,
        },
    },
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            FindWindowW, GetSystemMetrics, KillTimer, RegisterWindowMessageW, SetTimer,
            SM_CXSMICON, SM_CYSMICON, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_NCDESTROY,
            WM_SETTINGCHANGE, WM_TIMER, WM_WTSSESSION_CHANGE,
        },
    },
};

const SUBCLASS_ID: usize = 0x50325054;
const REFRESH_TIMER_ID: usize = 0x50325054;
const REFRESH_DELAY_MS: u32 = 500;

struct TrayRefresh {
    app: AppHandle,
    source: RgbaImage,
    taskbar_created: u32,
}

pub(super) fn install(window: &tauri::Window) -> Result<(), Box<dyn std::error::Error>> {
    let hwnd = window.hwnd()?.0 as HWND;
    let state = Box::new(TrayRefresh {
        app: window.app_handle(),
        source: image::load_from_memory(include_bytes!("../icons/128x128@2x.png"))?.into_rgba8(),
        taskbar_created: unsafe { RegisterWindowMessageW(windows_sys::w!("TaskbarCreated")) },
    });
    unsafe {
        let state = Box::into_raw(state);
        if SetWindowSubclass(hwnd, Some(window_proc), SUBCLASS_ID, state as usize) == 0 {
            drop(Box::from_raw(state));
            return Err(std::io::Error::last_os_error().into());
        }
        if WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) == 0 {
            tracing::warn!(
                "[p2premote] Failed to register tray session notifications: {}",
                std::io::Error::last_os_error()
            );
        }
        SetTimer(hwnd, REFRESH_TIMER_ID, REFRESH_DELAY_MS, None);
    }
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    data: usize,
) -> LRESULT {
    let state = &*(data as *const TrayRefresh);
    match message {
        WM_NCDESTROY => {
            KillTimer(hwnd, REFRESH_TIMER_ID);
            WTSUnRegisterSessionNotification(hwnd);
            RemoveWindowSubclass(hwnd, Some(window_proc), SUBCLASS_ID);
            drop(Box::from_raw(data as *mut TrayRefresh));
        }
        WM_TIMER if wparam == REFRESH_TIMER_ID => {
            KillTimer(hwnd, REFRESH_TIMER_ID);
            let (width, height) = taskbar_icon_size();
            if let Err(error) =
                state
                    .app
                    .tray_handle()
                    .set_icon(render_icon(&state.source, width, height))
            {
                tracing::warn!("[p2premote] Failed to refresh tray icon: {error}");
            }
            return 0;
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE | WM_WTSSESSION_CHANGE => {
            // RDP reconnects can deliver several display/session changes together.
            SetTimer(hwnd, REFRESH_TIMER_ID, REFRESH_DELAY_MS, None);
        }
        _ if state.taskbar_created != 0 && message == state.taskbar_created => {
            SetTimer(hwnd, REFRESH_TIMER_ID, REFRESH_DELAY_MS, None);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

fn render_icon(source: &RgbaImage, width: u32, height: u32) -> tauri::Icon {
    // Always resize the retained original, never the previous tray bitmap.
    let pixels = image::imageops::resize(source, width, height, FilterType::Lanczos3);
    tauri::Icon::Rgba {
        rgba: pixels.into_raw(),
        width,
        height,
    }
}

unsafe fn taskbar_icon_size() -> (u32, u32) {
    let user32 = GetModuleHandleW(windows_sys::w!("user32.dll"));
    // These APIs are absent on Windows 7, which uses the system metrics below.
    if !user32.is_null() {
        if let (Some(get_dpi), Some(get_metrics)) = (
            GetProcAddress(user32, c"GetDpiForWindow".as_ptr().cast()),
            GetProcAddress(user32, c"GetSystemMetricsForDpi".as_ptr().cast()),
        ) {
            let get_dpi: unsafe extern "system" fn(HWND) -> u32 = std::mem::transmute(get_dpi);
            let get_metrics: unsafe extern "system" fn(i32, u32) -> i32 =
                std::mem::transmute(get_metrics);
            let taskbar = FindWindowW(windows_sys::w!("Shell_TrayWnd"), std::ptr::null());
            let dpi = get_dpi(taskbar);
            if dpi != 0 {
                return (
                    get_metrics(SM_CXSMICON, dpi).max(1) as u32,
                    get_metrics(SM_CYSMICON, dpi).max(1) as u32,
                );
            }
        }
    }
    (
        GetSystemMetrics(SM_CXSMICON).max(1) as u32,
        GetSystemMetrics(SM_CYSMICON).max(1) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_size_changes_keep_the_original_pixels() {
        let source = image::load_from_memory(include_bytes!("../icons/128x128@2x.png"))
            .unwrap()
            .into_rgba8();
        let original = source.clone();
        let tauri::Icon::Rgba { rgba: first, .. } = render_icon(&source, 16, 16) else {
            panic!("expected RGBA icon")
        };
        for size in [24, 32, 20, 48, 16] {
            let tauri::Icon::Rgba {
                rgba,
                width,
                height,
            } = render_icon(&source, size, size)
            else {
                panic!("expected RGBA icon")
            };
            assert_eq!((width, height), (size, size));
            assert_eq!(rgba.len(), (size * size * 4) as usize);
            if size == 16 {
                assert_eq!(rgba, first);
            }
        }
        assert_eq!(source, original);
    }
}
