use enigo::{Enigo, Key, Keyboard, Mouse, Settings};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
#[cfg(target_os = "windows")]
use std::sync::OnceLock;
use tauri::{AppHandle, Manager};

static TARGET_INTERACTION_EPOCH: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "windows")]
static TARGET_INTERACTION_BARRIER: Mutex<()> = Mutex::new(());
#[cfg(target_os = "windows")]
static TARGET_MONITOR_STATE: AtomicU8 = AtomicU8::new(MONITOR_NOT_STARTED);
#[cfg(target_os = "windows")]
const MONITOR_NOT_STARTED: u8 = 0;
#[cfg(target_os = "windows")]
const MONITOR_STARTING: u8 = 1;
#[cfg(target_os = "windows")]
const MONITOR_READY: u8 = 2;
#[cfg(target_os = "windows")]
const MONITOR_FAILED: u8 = 3;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetGUIThreadInfo, GetMessageW, GetWindowThreadProcessId,
    SetWindowsHookExW, UnhookWindowsHookEx, GUITHREADINFO, MSG, WH_MOUSE_LL, WM_LBUTTONDOWN,
    WM_MBUTTONDOWN, WM_RBUTTONDOWN, WM_XBUTTONDOWN,
};

pub(crate) fn target_interaction_epoch() -> u64 {
    TARGET_INTERACTION_EPOCH.load(Ordering::Acquire)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VerifiedTargetError {
    MonitorUnavailable,
    TargetChanged,
    Action(String),
    PartialAction(String),
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn target_mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0
        && matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        )
    {
        // The matching insertion critical section contains only a foreground
        // identity read and one SendInput submission. It never contains text
        // generation, clipboard work, delivery waits, or sleeps, keeping this
        // hook wait bounded far below LowLevelHooksTimeout.
        {
            let _barrier = TARGET_INTERACTION_BARRIER
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            TARGET_INTERACTION_EPOCH.fetch_add(1, Ordering::AcqRel);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(target_os = "windows")]
fn mark_target_monitor_failed() {
    let _barrier = TARGET_INTERACTION_BARRIER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    TARGET_MONITOR_STATE.store(MONITOR_FAILED, Ordering::Release);
}

#[cfg(target_os = "windows")]
pub(crate) fn start_target_interaction_monitor() -> bool {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        TARGET_MONITOR_STATE.store(MONITOR_STARTING, Ordering::Release);
        let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel(1);
        let spawn_result = std::thread::Builder::new()
            .name("target-interaction-monitor".into())
            .spawn(move || unsafe {
                let hook = match SetWindowsHookExW(WH_MOUSE_LL, Some(target_mouse_hook), None, 0) {
                    Ok(hook) => hook,
                    Err(error) => {
                        log::error!("Failed to start target interaction monitor: {error}");
                        let _ = ready_sender.send(Err(error.to_string()));
                        return;
                    }
                };
                if ready_sender.send(Ok(())).is_err() {
                    mark_target_monitor_failed();
                    let _ = UnhookWindowsHookEx(hook);
                    return;
                }
                let mut message = MSG::default();
                loop {
                    let status = GetMessageW(&mut message, None, 0, 0).0;
                    if status > 0 {
                        continue;
                    }
                    if status < 0 {
                        log::error!(
                            "Target interaction monitor message loop failed: {}",
                            windows::core::Error::from_win32()
                        );
                    }
                    break;
                }
                mark_target_monitor_failed();
                let _ = UnhookWindowsHookEx(hook);
            });
        if let Err(error) = spawn_result {
            log::error!("Failed to spawn target interaction monitor: {error}");
            mark_target_monitor_failed();
            return;
        }
        match ready_receiver.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(Ok(())) => {
                let _ = TARGET_MONITOR_STATE.compare_exchange(
                    MONITOR_STARTING,
                    MONITOR_READY,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
            Ok(Err(error)) => {
                log::error!("Target interaction monitor unavailable: {error}");
                mark_target_monitor_failed();
            }
            Err(error) => {
                log::error!("Timed out starting target interaction monitor: {error}");
                mark_target_monitor_failed();
            }
        }
    });
    monitor_state_is_ready(TARGET_MONITOR_STATE.load(Ordering::Acquire))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn start_target_interaction_monitor() -> bool {
    false
}

#[cfg(target_os = "windows")]
fn monitor_state_is_ready(state: u8) -> bool {
    state == MONITOR_READY
}

#[cfg(target_os = "windows")]
pub(crate) fn target_interaction_monitor_ready() -> bool {
    monitor_state_is_ready(TARGET_MONITOR_STATE.load(Ordering::Acquire))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn target_interaction_monitor_ready() -> bool {
    false
}

#[cfg(target_os = "windows")]
pub(crate) fn capture_target_identity() -> Option<crate::progressive_dictation::TargetIdentity> {
    if !target_interaction_monitor_ready() {
        return None;
    }
    capture_target_identity_unchecked()
}

#[cfg(target_os = "windows")]
fn capture_target_identity_unchecked() -> Option<crate::progressive_dictation::TargetIdentity> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        return None;
    }

    let thread_id = unsafe { GetWindowThreadProcessId(foreground, None) };
    if thread_id == 0 {
        return None;
    }

    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetGUIThreadInfo(thread_id, &mut info) }.ok()?;
    if info.hwndFocus.0.is_null() {
        return None;
    }

    Some(crate::progressive_dictation::TargetIdentity::from_raw(
        foreground.0 as isize,
        info.hwndFocus.0 as isize,
        target_interaction_epoch(),
    ))
}

#[cfg(target_os = "windows")]
pub(crate) fn insert_text_at_verified_target(
    expected: crate::progressive_dictation::TargetIdentity,
    text: &str,
) -> Result<(), VerifiedTargetError> {
    let inputs = unicode_input_batch(text);
    let _barrier = TARGET_INTERACTION_BARRIER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !target_interaction_monitor_ready() {
        return Err(VerifiedTargetError::MonitorUnavailable);
    }
    if !capture_target_identity_unchecked().is_some_and(|current| expected.matches(current)) {
        return Err(VerifiedTargetError::TargetChanged);
    }
    send_unicode_input_batch(&inputs)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn insert_text_at_verified_target(
    _expected: crate::progressive_dictation::TargetIdentity,
    _text: &str,
) -> Result<(), VerifiedTargetError> {
    Err(VerifiedTargetError::MonitorUnavailable)
}

#[cfg(target_os = "windows")]
fn unicode_input_batch(text: &str) -> Vec<INPUT> {
    text.encode_utf16()
        .flat_map(|unit| {
            [
                INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: Default::default(),
                            wScan: unit,
                            dwFlags: KEYEVENTF_UNICODE,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                },
                INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: Default::default(),
                            wScan: unit,
                            dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                },
            ]
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn send_unicode_input_batch(inputs: &[INPUT]) -> Result<(), VerifiedTargetError> {
    if inputs.is_empty() {
        return Ok(());
    }
    let inserted = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if inserted == inputs.len() as u32 {
        return Ok(());
    }
    let message = format!(
        "SendInput inserted {inserted} of {} Unicode keyboard events",
        inputs.len()
    );
    if inserted == 0 {
        Err(VerifiedTargetError::Action(message))
    } else {
        Err(VerifiedTargetError::PartialAction(message))
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn capture_target_identity() -> Option<crate::progressive_dictation::TargetIdentity> {
    None
}

#[cfg(all(test, target_os = "windows"))]
mod target_monitor_tests {
    use super::{
        monitor_state_is_ready, MONITOR_FAILED, MONITOR_NOT_STARTED, MONITOR_READY,
        MONITOR_STARTING,
    };

    #[test]
    fn target_capture_is_allowed_only_after_monitor_is_ready() {
        assert!(!monitor_state_is_ready(MONITOR_NOT_STARTED));
        assert!(!monitor_state_is_ready(MONITOR_STARTING));
        assert!(monitor_state_is_ready(MONITOR_READY));
        assert!(!monitor_state_is_ready(MONITOR_FAILED));
    }

    #[test]
    fn unicode_batch_has_key_down_and_up_for_every_utf16_unit() {
        let inputs = super::unicode_input_batch("A🙂");
        assert_eq!(inputs.len(), 6);
        for pair in inputs.chunks_exact(2) {
            assert_eq!(pair[0].r#type, super::INPUT_KEYBOARD);
            assert_eq!(pair[1].r#type, super::INPUT_KEYBOARD);
            let down = unsafe { pair[0].Anonymous.ki };
            let up = unsafe { pair[1].Anonymous.ki };
            assert_eq!(down.dwFlags, super::KEYEVENTF_UNICODE);
            assert_eq!(
                up.dwFlags,
                super::KEYEVENTF_UNICODE | super::KEYEVENTF_KEYUP
            );
            assert_eq!(down.wScan, up.wScan);
        }
    }
}

/// Wrapper for Enigo to store in Tauri's managed state.
/// Enigo is wrapped in a Mutex since it requires mutable access.
pub struct EnigoState(pub Mutex<Enigo>);

impl EnigoState {
    pub fn new() -> Result<Self, String> {
        let enigo = Enigo::new(&Settings::default())
            .map_err(|e| format!("Failed to initialize Enigo: {}", e))?;
        Ok(Self(Mutex::new(enigo)))
    }
}

/// Get the current mouse cursor position using the managed Enigo instance.
/// Returns None if the state is not available or if getting the location fails.
pub fn get_cursor_position(app_handle: &AppHandle) -> Option<(i32, i32)> {
    let enigo_state = app_handle.try_state::<EnigoState>()?;
    let enigo = enigo_state.0.lock().ok()?;
    enigo.location().ok()
}

/// Sends a Ctrl+V or Cmd+V paste command using platform-specific virtual key codes.
/// This ensures the paste works regardless of keyboard layout (e.g., Russian, AZERTY, DVORAK).
/// Note: On Wayland, this may not work - callers should check for Wayland and use alternative methods.
///
/// `hold_ms` is how long the modifier stays held after the V click before being
/// released. Most applications read the modifier from the V event's flags and
/// need no hold at all, but applications that poll global keyboard state when
/// handling the key need the modifier to still be down — the hold insures
/// against those. Callers that can detect a failed chord (e.g. the
/// receipt-sequenced paste path) may use a much shorter hold.
pub fn send_paste_ctrl_v(enigo: &mut Enigo, hold_ms: u64) -> Result<(), String> {
    // Platform-specific key definitions
    #[cfg(target_os = "macos")]
    let (modifier_key, v_key_code) = (Key::Meta, Key::Other(9));
    #[cfg(target_os = "windows")]
    let (modifier_key, v_key_code) = (Key::Control, Key::Other(0x56)); // VK_V
    #[cfg(target_os = "linux")]
    let (modifier_key, v_key_code) = (Key::Control, Key::Unicode('v'));

    // Press modifier + V
    enigo
        .key(modifier_key, enigo::Direction::Press)
        .map_err(|e| format!("Failed to press modifier key: {}", e))?;
    enigo
        .key(v_key_code, enigo::Direction::Click)
        .map_err(|e| format!("Failed to click V key: {}", e))?;

    std::thread::sleep(std::time::Duration::from_millis(hold_ms));

    enigo
        .key(modifier_key, enigo::Direction::Release)
        .map_err(|e| format!("Failed to release modifier key: {}", e))?;

    Ok(())
}

/// Sends a Ctrl+Shift+V paste command.
/// This is commonly used in terminal applications on Linux to paste without formatting.
/// Note: On Wayland, this may not work - callers should check for Wayland and use alternative methods.
pub fn send_paste_ctrl_shift_v(enigo: &mut Enigo, hold_ms: u64) -> Result<(), String> {
    // Platform-specific key definitions
    #[cfg(target_os = "macos")]
    let (modifier_key, v_key_code) = (Key::Meta, Key::Other(9)); // Cmd+Shift+V on macOS
    #[cfg(target_os = "windows")]
    let (modifier_key, v_key_code) = (Key::Control, Key::Other(0x56)); // VK_V
    #[cfg(target_os = "linux")]
    let (modifier_key, v_key_code) = (Key::Control, Key::Unicode('v'));

    // Press Ctrl/Cmd + Shift + V
    enigo
        .key(modifier_key, enigo::Direction::Press)
        .map_err(|e| format!("Failed to press modifier key: {}", e))?;
    enigo
        .key(Key::Shift, enigo::Direction::Press)
        .map_err(|e| format!("Failed to press Shift key: {}", e))?;
    enigo
        .key(v_key_code, enigo::Direction::Click)
        .map_err(|e| format!("Failed to click V key: {}", e))?;

    std::thread::sleep(std::time::Duration::from_millis(hold_ms));

    enigo
        .key(Key::Shift, enigo::Direction::Release)
        .map_err(|e| format!("Failed to release Shift key: {}", e))?;
    enigo
        .key(modifier_key, enigo::Direction::Release)
        .map_err(|e| format!("Failed to release modifier key: {}", e))?;

    Ok(())
}

/// Sends a Shift+Insert paste command (Windows and Linux only).
/// This is more universal for terminal applications and legacy software.
/// Note: On Wayland, this may not work - callers should check for Wayland and use alternative methods.
pub fn send_paste_shift_insert(enigo: &mut Enigo, hold_ms: u64) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let insert_key_code = Key::Other(0x2D); // VK_INSERT
    #[cfg(not(target_os = "windows"))]
    let insert_key_code = Key::Other(0x76); // XK_Insert (keycode 118 / 0x76, also used as fallback)

    // Press Shift + Insert
    enigo
        .key(Key::Shift, enigo::Direction::Press)
        .map_err(|e| format!("Failed to press Shift key: {}", e))?;
    enigo
        .key(insert_key_code, enigo::Direction::Click)
        .map_err(|e| format!("Failed to click Insert key: {}", e))?;

    std::thread::sleep(std::time::Duration::from_millis(hold_ms));

    enigo
        .key(Key::Shift, enigo::Direction::Release)
        .map_err(|e| format!("Failed to release Shift key: {}", e))?;

    Ok(())
}

/// Pastes text directly using the enigo text method.
/// This tries to use system input methods if possible, otherwise simulates keystrokes one by one.
pub fn paste_text_direct(enigo: &mut Enigo, text: &str) -> Result<(), String> {
    enigo
        .text(text)
        .map_err(|e| format!("Failed to send text directly: {}", e))?;

    Ok(())
}
