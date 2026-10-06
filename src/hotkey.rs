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
use crate::window::{WM_KEY_CAPTURED, WM_KEY_DOWN, WM_KEY_UP};

/// wParam kind for WM_KEY_CAPTURED.
pub const CAP_CANCEL: usize = 0;
pub const CAP_KEY: usize = 1;
pub const CAP_MOUSE: usize = 2;

static mut HOOK_KB: Option<HHOOK> = None;
static mut HOOK_MS: Option<HHOOK> = None;
static mut TARGET: HWND = HWND(std::ptr::null_mut());
static mut BIND: ToggleKey = ToggleKey::None;
static mut CAPTURING: bool = false;
// True between a matched key-down and its key-up. Suppresses keyboard
// auto-repeat (holding a key re-fires WM_KEYDOWN) so a hold is one
// down/up pair, and a toggle fires once per press.
static mut DOWN_ACTIVE: bool = false;

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
        DOWN_ACTIVE = false;
    }
}

/// Refresh the shadow bind after cfg changes (called from wndproc only).
/// Resets the down tracker so a bind change mid-hold cannot stick it.
pub fn set_bind(bind: ToggleKey) {
    unsafe {
        BIND = bind;
        DOWN_ACTIVE = false;
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

fn post_down(kind: usize, code: usize) {
    unsafe {
        let _ = PostMessageW(
            Some(target()),
            WM_KEY_DOWN,
            WPARAM(kind),
            LPARAM(code as isize),
        );
    }
}

fn post_up(kind: usize, code: usize) {
    unsafe {
        let _ = PostMessageW(
            Some(target()),
            WM_KEY_UP,
            WPARAM(kind),
            LPARAM(code as isize),
        );
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
                // Suppress auto-repeat: one press = one down.
                if !DOWN_ACTIVE {
                    DOWN_ACTIVE = true;
                    post_down(CAP_KEY, vk as usize);
                }
            }
        } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
            // Key-up is ignored while capturing (capture is down-only).
            if !CAPTURING {
                let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
                let vk = info.vkCode as u16;
                if matches(BIND, CAP_KEY, vk as usize) {
                    DOWN_ACTIVE = false;
                    post_up(CAP_KEY, vk as usize);
                }
            }
        }
    }
    // Pass-through: never swallow, the key keeps working everywhere.
    CallNextHookEx(None, ncode, wparam, lparam)
}

fn xbutton_from_lparam(lparam: LPARAM) -> Option<MouseBtn> {
    unsafe {
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
}

unsafe extern "system" fn ms_proc(ncode: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if ncode >= 0 {
        let msg = wparam.0 as u32;
        // Split down vs up; X-button id comes from mouseData for both.
        let down_btn: Option<MouseBtn> = match msg {
            WM_LBUTTONDOWN => Some(MouseBtn::Left),
            WM_RBUTTONDOWN => Some(MouseBtn::Right),
            WM_MBUTTONDOWN => Some(MouseBtn::Middle),
            WM_XBUTTONDOWN => xbutton_from_lparam(lparam),
            _ => None,
        };
        let up_btn: Option<MouseBtn> = match msg {
            WM_LBUTTONUP => Some(MouseBtn::Left),
            WM_RBUTTONUP => Some(MouseBtn::Right),
            WM_MBUTTONUP => Some(MouseBtn::Middle),
            WM_XBUTTONUP => xbutton_from_lparam(lparam),
            _ => None,
        };
        if let Some(b) = down_btn {
            if CAPTURING {
                post_capture(CAP_MOUSE, b.id() as usize);
            } else if matches(BIND, CAP_MOUSE, b.id() as usize) {
                if !DOWN_ACTIVE {
                    DOWN_ACTIVE = true;
                    post_down(CAP_MOUSE, b.id() as usize);
                }
            }
        } else if let Some(b) = up_btn {
            if !CAPTURING && matches(BIND, CAP_MOUSE, b.id() as usize) {
                DOWN_ACTIVE = false;
                post_up(CAP_MOUSE, b.id() as usize);
            }
        }
    }
    // Pass-through: never swallow, the button keeps working everywhere.
    CallNextHookEx(None, ncode, wparam, lparam)
}
