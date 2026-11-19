//! Global hotkeys via low-level hooks (WH_KEYBOARD_LL + WH_MOUSE_LL).
//!
//! Design notes:
//! - The hook procs run on our UI thread (low-level hooks execute in the
//!   installing thread's context). They do O(n) matching over a small list of
//!   triggers and only PostMessage our window; all state changes happen in
//!   wndproc handlers. They NEVER touch APP, so there is no reentrancy hazard.
//! - Events are always passed through (CallNextHookEx): a bound key keeps
//!   its normal function in games/apps AND runs its OnAir action.
//! - Mouse coverage is whatever Windows itself sees: Left/Right/Middle/X1/X2.
//!   Extra vendor buttons are invisible to low-level hooks OS-wide.
//! - Trigger modifiers are matched exactly: `Ctrl+F13` fires only while Ctrl
//!   is held, and a bare `F13` trigger does not fire while Ctrl is held. This
//!   lets one key host several macros.

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{normalize_vk, Modifiers, MouseBtn, ToggleKey, Trigger};
use crate::window::{WM_KEY_CAPTURED, WM_KEY_DOWN, WM_KEY_UP};

/// wParam kind for WM_KEY_CAPTURED. Modifiers are packed into the high byte.
pub const CAP_CANCEL: usize = 0;
pub const CAP_KEY: usize = 1;
pub const CAP_MOUSE: usize = 2;

/// Action id used by WM_KEY_DOWN/WM_KEY_UP. 0 is the overlay mic hotkey;
/// macro `i` uses `1 + i`.
pub const ACTION_TOGGLE: usize = 0;

pub fn action_macro(index: usize) -> usize {
    index + 1
}

fn mods_bits(mods: Modifiers) -> usize {
    (mods.ctrl as usize)
        | ((mods.alt as usize) << 1)
        | ((mods.shift as usize) << 2)
        | ((mods.win as usize) << 3)
}

/// Decode the modifier bits packed into WM_KEY_CAPTURED's wParam.
pub fn captured_mods(wparam: usize) -> Modifiers {
    let bits = wparam >> 8;
    Modifiers {
        ctrl: bits & 1 != 0,
        alt: bits & 2 != 0,
        shift: bits & 4 != 0,
        win: bits & 8 != 0,
    }
}

pub fn captured_kind(wparam: usize) -> usize {
    wparam & 0xFF
}

static mut HOOK_KB: Option<HHOOK> = None;
static mut HOOK_MS: Option<HHOOK> = None;
static mut TARGET: HWND = HWND(std::ptr::null_mut());
static mut BIND: Trigger = Trigger::NONE;
static mut MACROS: Vec<Trigger> = Vec::new();
static mut CAPTURING: bool = false;
// Modifier state as of the last hook event. Low-level hooks always see the
// modifier down/up pair before the key that uses them.
static mut MODS: Modifiers = Modifiers {
    ctrl: false,
    alt: false,
    shift: false,
    win: false,
};
// True between a matched action's key-down and its key-up. Suppresses
// keyboard auto-repeat (holding a key re-fires WM_KEYDOWN) so a hold is one
// down/up pair, and a press fires once per physical press.
static mut DOWN_TOGGLE: bool = false;
static mut DOWN_MACROS: Vec<bool> = Vec::new();

fn target() -> HWND {
    unsafe { TARGET }
}

/// Install only the hooks that are not installed yet. Called lazily when a
/// bind exists or capture starts, so an unbound app has no global hooks.
fn ensure_hooks() {
    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        let hinstance = HINSTANCE(module.0);
        if HOOK_KB.is_none() {
            HOOK_KB = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), Some(hinstance), 0).ok();
        }
        if HOOK_MS.is_none() {
            HOOK_MS = SetWindowsHookExW(WH_MOUSE_LL, Some(ms_proc), Some(hinstance), 0).ok();
        }
    }
}

fn remove_hooks() {
    unsafe {
        if let Some(h) = HOOK_KB.take() {
            let _ = UnhookWindowsHookEx(h);
        }
        if let Some(h) = HOOK_MS.take() {
            let _ = UnhookWindowsHookEx(h);
        }
        DOWN_TOGGLE = false;
        DOWN_MACROS.iter_mut().for_each(|down| *down = false);
    }
}

fn has_binds() -> bool {
    unsafe { !BIND.is_none() || !MACROS.is_empty() }
}

/// Install hooks only if at least one trigger is configured. Safe to call
/// once at startup; on failure the hotkeys simply stay inactive (no popups).
pub fn init(hwnd: HWND, _hinstance: HINSTANCE, bind: Trigger, macros: Vec<Trigger>) {
    unsafe {
        TARGET = hwnd;
        BIND = bind;
        MACROS = macros;
        DOWN_MACROS = vec![false; MACROS.len()];
    }
    if has_binds() {
        ensure_hooks();
    }
}

pub fn shutdown() {
    remove_hooks();
    unsafe {
        CAPTURING = false;
    }
}

/// Refresh the shadow triggers after cfg changes (called from wndproc only).
/// Resets the down trackers so a bind change mid-hold cannot stick them.
pub fn set_bind(bind: Trigger, macros: Vec<Trigger>) {
    unsafe {
        BIND = bind;
        MACROS = macros;
        DOWN_TOGGLE = false;
        DOWN_MACROS = vec![false; MACROS.len()];
    }
    if !has_binds() && !is_capturing() {
        remove_hooks();
    } else {
        ensure_hooks();
    }
}

pub fn set_capturing(on: bool) {
    unsafe {
        CAPTURING = on;
    }
    if on {
        ensure_hooks();
    } else if !has_binds() {
        remove_hooks();
    }
}

fn modifier_flag(vk: u16) -> Option<fn(&mut Modifiers, bool)> {
    match normalize_vk(vk) {
        0x10 => Some(|m, on| m.shift = on),
        0x11 => Some(|m, on| m.ctrl = on),
        0x12 => Some(|m, on| m.alt = on),
        0x5B | 0x5C => Some(|m, on| m.win = on),
        _ => None,
    }
}

pub fn is_capturing() -> bool {
    unsafe { CAPTURING }
}

fn post_action(msg: u32, action: usize) {
    unsafe {
        let _ = PostMessageW(Some(target()), msg, WPARAM(action), LPARAM(0));
    }
}

fn post_capture(kind: usize, code: usize, mods: Modifiers) {
    unsafe {
        let _ = PostMessageW(
            Some(target()),
            WM_KEY_CAPTURED,
            WPARAM(kind | (mods_bits(mods) << 8)),
            LPARAM(code as isize),
        );
    }
}

fn key_matches(trigger: Trigger, kind: usize, code: usize) -> bool {
    match trigger.key {
        ToggleKey::None => false,
        ToggleKey::Key(vk) => kind == CAP_KEY && code == vk as usize,
        ToggleKey::Mouse(b) => kind == CAP_MOUSE && code == b.id() as usize,
    }
}

fn trigger_matches(trigger: Trigger, kind: usize, code: usize, mods: Modifiers) -> bool {
    trigger.mods == mods && key_matches(trigger, kind, code)
}

unsafe extern "system" fn kb_proc(ncode: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if ncode >= 0 {
        let msg = wparam.0 as u32;
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let vk = normalize_vk(info.vkCode as u16);
            if CAPTURING {
                // Esc cancels capture and is never bindable. Modifiers are
                // recorded and skipped so Ctrl+A captures Ctrl+A.
                if vk == 0x1B {
                    post_capture(CAP_CANCEL, 0, Modifiers::default());
                } else if let Some(flag) = modifier_flag(vk) {
                    flag(&mut MODS, true);
                } else {
                    post_capture(CAP_KEY, vk as usize, MODS);
                }
            } else if let Some(flag) = modifier_flag(vk) {
                flag(&mut MODS, true);
            } else {
                if !DOWN_TOGGLE && trigger_matches(BIND, CAP_KEY, vk as usize, MODS) {
                    DOWN_TOGGLE = true;
                    post_action(WM_KEY_DOWN, ACTION_TOGGLE);
                }
                for (index, trigger) in MACROS.iter().enumerate() {
                    if !DOWN_MACROS[index] && trigger_matches(*trigger, CAP_KEY, vk as usize, MODS)
                    {
                        DOWN_MACROS[index] = true;
                        post_action(WM_KEY_DOWN, action_macro(index));
                    }
                }
            }
        } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
            // Key-up is ignored while capturing (capture is down-only).
            if !CAPTURING {
                let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
                let vk = normalize_vk(info.vkCode as u16);
                if let Some(flag) = modifier_flag(vk) {
                    flag(&mut MODS, false);
                }
                // Match only the base key on release: the user may let go of
                // Ctrl before F13, and the hold must still end cleanly.
                if DOWN_TOGGLE && key_matches(BIND, CAP_KEY, vk as usize) {
                    DOWN_TOGGLE = false;
                    post_action(WM_KEY_UP, ACTION_TOGGLE);
                }
                for (index, trigger) in MACROS.iter().enumerate() {
                    if DOWN_MACROS[index] && key_matches(*trigger, CAP_KEY, vk as usize) {
                        DOWN_MACROS[index] = false;
                        post_action(WM_KEY_UP, action_macro(index));
                    }
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
            let code = b.id() as usize;
            if CAPTURING {
                post_capture(CAP_MOUSE, code, MODS);
            } else {
                if !DOWN_TOGGLE && trigger_matches(BIND, CAP_MOUSE, code, MODS) {
                    DOWN_TOGGLE = true;
                    post_action(WM_KEY_DOWN, ACTION_TOGGLE);
                }
                for (index, trigger) in MACROS.iter().enumerate() {
                    if !DOWN_MACROS[index] && trigger_matches(*trigger, CAP_MOUSE, code, MODS) {
                        DOWN_MACROS[index] = true;
                        post_action(WM_KEY_DOWN, action_macro(index));
                    }
                }
            }
        } else if let Some(b) = up_btn {
            let code = b.id() as usize;
            if !CAPTURING {
                if DOWN_TOGGLE && key_matches(BIND, CAP_MOUSE, code) {
                    DOWN_TOGGLE = false;
                    post_action(WM_KEY_UP, ACTION_TOGGLE);
                }
                for (index, trigger) in MACROS.iter().enumerate() {
                    if DOWN_MACROS[index] && key_matches(*trigger, CAP_MOUSE, code) {
                        DOWN_MACROS[index] = false;
                        post_action(WM_KEY_UP, action_macro(index));
                    }
                }
            }
        }
    }
    // Pass-through: never swallow, the button keeps working everywhere.
    CallNextHookEx(None, ncode, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_modifiers_are_not_capturable() {
        for vk in [
            0x10, 0x11, 0x12, 0x5B, 0x5C, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5,
        ] {
            assert!(
                modifier_flag(vk).is_some(),
                "VK {vk:#04X} should be a modifier"
            );
        }
        for vk in [0x1B, 0x20, 0x41, 0x70, 0x7C] {
            assert!(
                modifier_flag(vk).is_none(),
                "VK {vk:#04X} should be bindable"
            );
        }
    }

    #[test]
    fn capture_modifier_bits_roundtrip() {
        let mods = Modifiers {
            ctrl: true,
            alt: false,
            shift: true,
            win: true,
        };
        let packed = CAP_KEY | (mods_bits(mods) << 8);
        assert_eq!(captured_kind(packed), CAP_KEY);
        assert_eq!(captured_mods(packed), mods);
        assert_eq!(captured_kind(CAP_CANCEL), CAP_CANCEL);
    }

    #[test]
    fn modifier_flags_track_both_sides() {
        let mut mods = Modifiers::default();
        for vk in [0xA0, 0xA1] {
            let flag = modifier_flag(vk).expect("shift");
            flag(&mut mods, true);
            assert!(mods.shift);
            flag(&mut mods, false);
            assert!(!mods.shift);
        }
        for vk in [0xA2, 0xA3] {
            let flag = modifier_flag(vk).expect("ctrl");
            flag(&mut mods, true);
            assert!(mods.ctrl);
            flag(&mut mods, false);
            assert!(!mods.ctrl);
        }
        assert!(modifier_flag(0x41).is_none());
    }

    #[test]
    fn exact_modifier_matching() {
        let bare = Trigger {
            key: ToggleKey::Key(0x7C),
            mods: Modifiers::default(),
        };
        let combo = Trigger {
            key: ToggleKey::Key(0x7C),
            mods: Modifiers {
                ctrl: true,
                ..Default::default()
            },
        };
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert!(trigger_matches(bare, CAP_KEY, 0x7C, Modifiers::default()));
        assert!(!trigger_matches(bare, CAP_KEY, 0x7C, ctrl));
        assert!(trigger_matches(combo, CAP_KEY, 0x7C, ctrl));
        assert!(!trigger_matches(combo, CAP_KEY, 0x7C, Modifiers::default()));
        // Mouse triggers use the same exact-modifier rule.
        let mouse = Trigger {
            key: ToggleKey::Mouse(MouseBtn::X1),
            mods: Modifiers::default(),
        };
        assert!(trigger_matches(mouse, CAP_MOUSE, 3, Modifiers::default()));
        assert!(!trigger_matches(mouse, CAP_KEY, 3, Modifiers::default()));
    }

    #[test]
    fn action_ids_are_stable() {
        assert_eq!(ACTION_TOGGLE, 0);
        assert_eq!(action_macro(0), 1);
        assert_eq!(action_macro(7), 8);
    }
}
