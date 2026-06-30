//! Detection of whether a fullscreen game (or other fullscreen app) is running.
//!
//! Used by the transcription idle-watcher to automatically unload the speech
//! model and free GPU VRAM while the user is gaming, then lazily reload it on
//! the next dictation.

/// Returns `true` when a fullscreen game / immersive app currently owns the
/// screen, indicating Handy should release the GPU.
#[cfg(target_os = "windows")]
pub fn is_game_running() -> bool {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::Shell::{
        SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetShellWindow, GetWindowRect,
    };

    unsafe {
        // 1) High-confidence signal: Windows reports a full-screen Direct3D app
        //    (exclusive-fullscreen game) or presentation mode is active. This is
        //    the same state the OS uses to suppress notifications.
        if let Ok(state) = SHQueryUserNotificationState() {
            if state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE {
                return true;
            }
        }

        // 2) Borderless-windowed games don't always set the D3D flag, so also
        //    treat "the foreground window covers the entire monitor" as
        //    fullscreen. A normally-maximized window leaves the taskbar visible
        //    (it fills the work area, not the whole monitor), so this cleanly
        //    distinguishes true fullscreen from maximized.
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return false;
        }
        // Ignore the desktop/shell window, whose rect also spans the monitor.
        if hwnd == GetShellWindow() {
            return false;
        }

        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return false;
        }

        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }

        let m = info.rcMonitor;
        rect.left <= m.left && rect.top <= m.top && rect.right >= m.right && rect.bottom >= m.bottom
    }
}

/// Game detection is currently Windows-only; other platforms never auto-unload.
#[cfg(not(target_os = "windows"))]
pub fn is_game_running() -> bool {
    false
}
