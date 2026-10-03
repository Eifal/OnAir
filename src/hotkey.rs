//! Global toggle keybind via low-level hooks (WH_KEYBOARD_LL + WH_MOUSE_LL).
//!
//! Design notes:
//! - The hook procs run on our UI thread (low-level hooks execute in the
//!   installing thread's context). They do O(1) matching and only
//!   PostMessage our window; all state changes happen in wndproc handlers.
//!   They NEVER touch APP, so there is no reentrancy hazard.
//! - Events are always passed through (CallNextHookEx): the bound key keeps
//!   its normal function in games/apps AND toggles the mic.
//! - Mouse coverage is whatever Windows itself sees: Left/Right/Middle/X1/X2.
//!   Extra vendor buttons are invisible to low-level hooks OS-wide.

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{MouseBtn, ToggleKey};
use crate::window::{WM_KEY_CAPTURED, WM_TOGGLE_REQ};

/// wParam kind for WM_KEY_CAPTURED.
pub const CAP_CANCEL: usize = 0;
pub const CAP_KEY: usize = 1;
pub const CAP_MOUSE: usize = 2;

static mut HOOK_KB: Option<HHOOK> = None;
static mut HOOK_MS: Option<HHOOK> = None;
static mut TARGET: HWND = HWND(std::ptr::null_mut());
static mut BIND: ToggleKey = ToggleKey::None;
static mut CAPTURING: bool = false;

fn target() -> HWND {
    unsafe { TARGET }
}

/// Install both hooks. Safe to call once at startup; on failure the
/// toggle feature simply stays inactive (no popups, per robustness rules).
pub fn init(hwnd: HWND, hinstance: HINSTANCE, bind: ToggleKey) {
    unsafe {
        TARGET = hwnd;
        BIND = bind;
        if HOOK_KB.is_none() {
            HOOK_KB = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), Some(hinstance), 0).ok();
        }
        if HOOK_MS.is_none() {
            HOOK_MS = SetWindowsHookExW(WH_MOUSE_LL, Some(ms_proc), Some(hinstance), 0).ok();
        }
    }
}

pub fn shutdown() {
    unsafe {
        if let Some(h) = HOOK_KB.take() {
            let _ = UnhookWindowsHookEx(h);
        }
        if let Some(h) = HOOK_MS.take() {
            let _ = UnhookWindowsHookEx(h);
        }
        CAPTURING = false;
    }
}

/// Refresh the shadow bind after cfg changes (called from wndproc only).
pub fn set_bind(bind: ToggleKey) {
    unsafe {
        BIND = bind;
    }
}

pub fn set_capturing(on: bool) {
    unsafe {
        CAPTURING = on;
    }
}

pub fn is_capturing() -> bool {
    unsafe { CAPTURING }
}

fn post_toggle() {
    unsafe {
        let _ = PostMessageW(Some(target()), WM_TOGGLE_REQ, WPARAM(0), LPARAM(0));
    }
}

fn post_capture(kind: usize, code: usize) {
    unsafe {
        let _ = PostMessageW(
            Some(target()),
            WM_KEY_CAPTURED,
            WPARAM(kind),
            LPARAM(code as isize),
        );
    }
}

fn matches(bind: ToggleKey, kind: usize, code: usize) -> bool {
    match bind {
        ToggleKey::None => false,
        ToggleKey::Key(vk) => kind == CAP_KEY && code == vk as usize,
        ToggleKey::Mouse(b) => kind == CAP_MOUSE && code == b.id() as usize,
    }
}

unsafe extern "system" fn kb_proc(ncode: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if ncode >= 0 {
        let msg = wparam.0 as u32;
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let vk = info.vkCode as u16;
            if CAPTURING {
                // Esc cancels capture and is never bindable.
                if vk == 0x1B {
                    post_capture(CAP_CANCEL, 0);
                } else {
                    post_capture(CAP_KEY, vk as usize);
                }
            } else if matches(BIND, CAP_KEY, vk as usize) {
                post_toggle();
            }
        }
    }
    // Pass-through: never swallow, the key keeps working everywhere.
    CallNextHookEx(None, ncode, wparam, lparam)
}

unsafe extern "system" fn ms_proc(ncode: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if ncode >= 0 {
        let msg = wparam.0 as u32;
        let btn: Option<MouseBtn> = match msg {
            WM_LBUTTONDOWN => Some(MouseBtn::Left),
            WM_RBUTTONDOWN => Some(MouseBtn::Right),
            WM_MBUTTONDOWN => Some(MouseBtn::Middle),
            WM_XBUTTONDOWN => {
                let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
                let x = ((info.mouseData >> 16) & 0xFFFF) as u16;
                if x == 1 {
                    Some(MouseBtn::X1)
                } else if x == 2 {
                    Some(MouseBtn::X2)
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(b) = btn {
            if CAPTURING {
                post_capture(CAP_MOUSE, b.id() as usize);
            } else if matches(BIND, CAP_MOUSE, b.id() as usize) {
                post_toggle();
            }
        }
    }
    // Pass-through: never swallow, the button keeps working everywhere.
    CallNextHookEx(None, ncode, wparam, lparam)
}
