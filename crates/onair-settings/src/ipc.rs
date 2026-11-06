use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

const OVERLAY_CLASS: &str = "OnAirOverlay";
const RELOAD_MSG: &str = "OnAir.ReloadConfig";
const BEGIN_CAPTURE_MSG: &str = "OnAir.BeginCapture";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn find_overlay() -> Option<HWND> {
    let class = wide(OVERLAY_CLASS);
    unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()).ok() }
}

pub fn overlay_running() -> bool {
    find_overlay().is_some()
}

pub fn notify_reload() -> bool {
    post_to_overlay(RELOAD_MSG)
}

#[allow(dead_code)]
pub fn notify_begin_capture() -> bool {
    post_to_overlay(BEGIN_CAPTURE_MSG)
}

fn post_to_overlay(name: &str) -> bool {
    let Some(hwnd) = find_overlay() else {
        return false;
    };
    let msg_name = wide(name);
    let msg = unsafe { RegisterWindowMessageW(PCWSTR(msg_name.as_ptr())) };
    if msg == 0 {
        return false;
    }
    unsafe { PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0)).is_ok() }
}
