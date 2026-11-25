//! Global hotkeys via low-level hooks (WH_KEYBOARD_LL + WH_MOUSE_LL).
//!
//! Design notes:
//! - The hooks live on their own thread ("onair-hotkeys") with a minimal
//!   message loop. Windows delivers a low-level hook call *through the
//!   installing thread*: while that thread is busy the event is dropped, and a
//!   hook proc that blocks longer than `LowLevelHooksTimeout` (300 ms by
//!   default) gets removed silently. Keeping Voicemeeter polling, painting,
//!   tray menus and config writes on the UI thread means a slow poll can never
//!   swallow a press or kill the hook.
//! - Hook procs only match against a small trigger list and PostMessage our
//!   window; no file I/O, no Remote API, no dialogs. All state changes happen
//!   in wndproc handlers.
//! - Modifiers are read from the live keyboard state on every event instead of
//!   being accumulated from down/up events. A missed key-up (lock screen, UAC,
//!   RDP reconnect, a stalled hook) used to leave Ctrl "held" forever, which
//!   silently stopped every unmodified trigger from matching until the app was
//!   restarted.
//! - A key/button counts as released when the OS reports it up, so a dropped
//!   key-up cannot wedge a trigger in the "still down" state either.
//! - Events are always passed through (CallNextHookEx): a bound key keeps its
//!   normal function in games/apps AND runs its OnAir action.
//! - The hooks are re-installed periodically. There is no API to ask whether a
//!   low-level hook was removed, so a fresh install is the only way back.
//! - Mouse coverage is whatever Windows itself sees: Left/Right/Middle/X1/X2.
//!   Extra vendor buttons are invisible to low-level hooks OS-wide.
//! - Trigger modifiers are matched exactly: `Ctrl+F13` fires only while Ctrl
//!   is held, and a bare `F13` trigger does not fire while Ctrl is held. This
//!   lets one key host several macros.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT,
    VK_RWIN,
};
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

/// Private message: apply a config change to the hook thread right away
/// instead of waiting for the next periodic health check.
const WM_HOTKEY_SYNC: u32 = WM_APP + 0x51;
/// Thread timer id for the periodic hook health check.
const TIMER_HOOK_CHECK: usize = 0x0A11;
/// Re-install the low-level hooks this often (seconds). Cheap, and the only
/// way to recover a hook that Windows removed without telling us.
const HOOK_REFRESH_MS: u32 = 30_000;

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

struct State {
    /// Main window handle as `isize` so the shared state stays `Sync`.
    target: isize,
    bind: Trigger,
    macros: Vec<Trigger>,
    guard_ms: u32,
    capturing: bool,
    /// `Some` while a matched press is down: the instant it started. Cleared on
    /// release, on a bind change, and when a new press arrives while the OS
    /// reports the key as up (the key-up was lost).
    down_toggle: Option<Instant>,
    down_macros: Vec<Option<Instant>>,
    last_toggle: Option<Instant>,
    last_macros: Vec<Option<Instant>>,
    thread_id: u32,
}

impl State {
    fn new() -> Self {
        Self {
            target: 0,
            bind: Trigger::NONE,
            macros: Vec::new(),
            guard_ms: 0,
            capturing: false,
            down_toggle: None,
            down_macros: Vec::new(),
            last_toggle: None,
            last_macros: Vec::new(),
            thread_id: 0,
        }
    }

    fn has_binds(&self) -> bool {
        !self.bind.is_none() || !self.macros.is_empty()
    }

    fn wanted(&self) -> bool {
        self.has_binds() || self.capturing
    }

    fn reset_downs(&mut self) {
        self.down_toggle = None;
        self.down_macros = vec![None; self.macros.len()];
        self.last_toggle = None;
        self.last_macros = vec![None; self.macros.len()];
    }
}

/// Shared between the UI thread (config updates, `is_capturing`) and the hook
/// thread (matching). Every critical section is a few field assignments.
fn state() -> MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE
        .get_or_init(|| Mutex::new(State::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn start_thread() {
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    match std::thread::Builder::new()
        .name("onair-hotkeys".to_string())
        .spawn(hook_thread)
    {
        Ok(_) => crate::dbglog!("hotkey thread spawned"),
        Err(err) => {
            SPAWNED.store(false, Ordering::SeqCst);
            crate::dbglog!("hotkey thread failed: {err}");
        }
    }
}

/// Tell the hook thread to re-check the desired hook state now.
fn wake() {
    let tid = state().thread_id;
    if tid != 0 {
        unsafe {
            let _ = PostThreadMessageW(tid, WM_HOTKEY_SYNC, WPARAM(0), LPARAM(0));
        }
    }
}

/// Install hooks only if at least one trigger is configured. Safe to call
/// once at startup; on failure the hotkeys stay inactive but are retried by
/// the periodic health check (no popups).
pub fn init(hwnd: HWND, _hinstance: HINSTANCE, bind: Trigger, macros: Vec<Trigger>, guard_ms: u32) {
    {
        let mut st = state();
        st.target = hwnd.0 as isize;
        st.bind = bind;
        st.macros = macros;
        st.guard_ms = guard_ms;
        st.reset_downs();
    }
    start_thread();
}

pub fn shutdown() {
    let tid = {
        let mut st = state();
        st.capturing = false;
        st.thread_id
    };
    if tid != 0 {
        unsafe {
            let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

/// Refresh the shadow triggers after cfg changes (called from wndproc only).
/// Resets the down trackers so a bind change mid-hold cannot stick them.
pub fn set_bind(bind: Trigger, macros: Vec<Trigger>, guard_ms: u32) {
    {
        let mut st = state();
        st.bind = bind;
        st.macros = macros;
        st.guard_ms = guard_ms;
        st.reset_downs();
    }
    start_thread();
    wake();
}

pub fn set_capturing(on: bool) {
    state().capturing = on;
    if on {
        start_thread();
    }
    wake();
}

pub fn is_capturing() -> bool {
    state().capturing
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

fn vk_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) < 0 }
}

/// Modifiers as they are right now, straight from the OS. Never accumulated,
/// so a missed key-up cannot leave a modifier stuck.
fn live_mods() -> Modifiers {
    Modifiers {
        ctrl: vk_down(VK_LCONTROL.0) || vk_down(VK_RCONTROL.0),
        alt: vk_down(VK_LMENU.0) || vk_down(VK_RMENU.0),
        shift: vk_down(VK_LSHIFT.0) || vk_down(VK_RSHIFT.0),
        win: vk_down(VK_LWIN.0) || vk_down(VK_RWIN.0),
    }
}

/// Virtual-key code for a mouse trigger id, for `GetAsyncKeyState`.
fn mouse_vk_code(btn: MouseBtn) -> u16 {
    match btn {
        MouseBtn::Left => 0x01,   // VK_LBUTTON
        MouseBtn::Right => 0x02,  // VK_RBUTTON
        MouseBtn::Middle => 0x04, // VK_MBUTTON
        MouseBtn::X1 => 0x05,     // VK_XBUTTON1
        MouseBtn::X2 => 0x06,     // VK_XBUTTON2
    }
}

/// Low-level hooks report the NumLock-off function for numpad keys: Numpad0
/// arrives as VK_INSERT (scan 0x52), Numpad4 as VK_LEFT, and so on. Map the
/// physical numpad scancode back to VK_NUMPADx so a captured key is labelled
/// and matched as the numpad key the user pressed, with NumLock on or off.
/// Dedicated navigation keys use extended scancodes and keep their own VK.
fn normalize_numpad(vk: u16, scan: u32, extended: bool) -> u16 {
    if extended {
        return vk;
    }
    match scan {
        0x47 => 0x67, // Num7 / Home
        0x48 => 0x68, // Num8 / Up
        0x49 => 0x69, // Num9 / PageUp
        0x4B => 0x64, // Num4 / Left
        0x4C => 0x65, // Num5 / Clear
        0x4D => 0x66, // Num6 / Right
        0x4F => 0x61, // Num1 / End
        0x50 => 0x62, // Num2 / Down
        0x51 => 0x63, // Num3 / PageDown
        0x52 => 0x60, // Num0 / Insert
        0x53 => 0x6E, // Num. / Delete
        _ => vk,
    }
}

/// Virtual key for a hook event, with left/right modifiers folded and numpad
/// keys resolved from their scancode.
fn event_vk(info: &KBDLLHOOKSTRUCT) -> u16 {
    normalize_numpad(
        normalize_vk(info.vkCode as u16),
        info.scanCode,
        info.flags.contains(LLKHF_EXTENDED),
    )
}

fn target(st: &State) -> HWND {
    HWND(st.target as *mut std::ffi::c_void)
}

fn post_action(st: &State, msg: u32, action: usize) {
    unsafe {
        let _ = PostMessageW(Some(target(st)), msg, WPARAM(action), LPARAM(0));
    }
}

fn post_capture(st: &State, kind: usize, code: usize, mods: Modifiers) {
    unsafe {
        let _ = PostMessageW(
            Some(target(st)),
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

/// True when this physical key/button drives any configured trigger, ignoring
/// modifiers. Only used to keep the debug log focused on relevant events.
fn bound_key(st: &State, kind: usize, code: usize) -> bool {
    key_matches(st.bind, kind, code) || st.macros.iter().any(|t| key_matches(*t, kind, code))
}

/// A tracked press whose key-up was missed. `GetAsyncKeyState` in a low-level
/// hook reports the state *before* the current event, which is exactly what we
/// need: for auto-repeat (key genuinely held) it still reports down, so the
/// hold is not broken.
fn is_stale(down: Option<Instant>, physically_down: bool) -> bool {
    down.is_some() && !physically_down
}

fn still_held(kind: usize, code: usize, raw_vk: u16) -> bool {
    let vk = match kind {
        CAP_KEY => raw_vk,
        CAP_MOUSE => match MouseBtn::from_id(code as u8) {
            Some(btn) => mouse_vk_code(btn),
            None => return true,
        },
        _ => return true,
    };
    vk_down(vk)
}

/// Drop a "still down" flag left behind by a key-up we never saw (window
/// locked, UAC, hook removed mid-hold, ...). Without this a single missed
/// release would make the trigger look permanently held and ignore every
/// later press.
fn clear_stale(st: &mut State, kind: usize, code: usize, raw_vk: u16) {
    if !is_stale(st.down_toggle, still_held(kind, code, raw_vk)) {
        return;
    }
    let toggle = st.down_toggle.is_some() && key_matches(st.bind, kind, code);
    let macro_stuck = (0..st.down_macros.len())
        .any(|i| st.down_macros[i].is_some() && key_matches(st.macros[i], kind, code));
    if !toggle && !macro_stuck {
        return;
    }
    if toggle {
        st.down_toggle = None;
    }
    for i in 0..st.down_macros.len() {
        if st.down_macros[i].is_some() && key_matches(st.macros[i], kind, code) {
            st.down_macros[i] = None;
        }
    }
    crate::dbglog!("released a stale press (key-up was missed)");
}

/// Re-trigger guard. Records the accepted press time and rejects a second
/// press inside `guard_ms` (mouse chatter / duplicated injected clicks).
fn retrigger_ok(last: &mut Option<Instant>, guard_ms: u32) -> bool {
    let now = Instant::now();
    let allowed = guard_allows(*last, now, guard_ms);
    if allowed {
        *last = Some(now);
    }
    allowed
}

/// Pure form of the guard, so the window logic is unit-testable.
fn guard_allows(last: Option<Instant>, now: Instant, guard_ms: u32) -> bool {
    if guard_ms == 0 {
        return true;
    }
    match last {
        Some(prev) => now.duration_since(prev).as_millis() >= guard_ms as u128,
        None => true,
    }
}

fn trigger_down(st: &mut State, kind: usize, code: usize, mods: Modifiers, raw_vk: u16) {
    clear_stale(st, kind, code, raw_vk);
    if st.down_toggle.is_none() && trigger_matches(st.bind, kind, code, mods) {
        if retrigger_ok(&mut st.last_toggle, st.guard_ms) {
            st.down_toggle = Some(Instant::now());
            post_action(st, WM_KEY_DOWN, ACTION_TOGGLE);
        } else {
            crate::dbglog!("trigger suppressed: retrigger guard");
        }
    }
    for index in 0..st.down_macros.len() {
        let trigger = st.macros[index];
        if st.down_macros[index].is_none() && trigger_matches(trigger, kind, code, mods) {
            if retrigger_ok(&mut st.last_macros[index], st.guard_ms) {
                st.down_macros[index] = Some(Instant::now());
                post_action(st, WM_KEY_DOWN, action_macro(index));
            } else {
                crate::dbglog!("macro {index} suppressed: retrigger guard");
            }
        }
    }
}

fn trigger_up(st: &mut State, kind: usize, code: usize) {
    if st.down_toggle.is_some() && key_matches(st.bind, kind, code) {
        st.down_toggle = None;
        post_action(st, WM_KEY_UP, ACTION_TOGGLE);
    }
    for index in 0..st.down_macros.len() {
        if st.down_macros[index].is_some() && key_matches(st.macros[index], kind, code) {
            st.down_macros[index] = None;
            post_action(st, WM_KEY_UP, action_macro(index));
        }
    }
}

fn kb_event(msg: u32, lparam: LPARAM) {
    let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
    if !down && !up {
        return;
    }
    let vk = event_vk(info);
    let mut st = state();

    if down {
        let mods = live_mods();
        if crate::debuglog::enabled() && (st.capturing || bound_key(&st, CAP_KEY, vk as usize)) {
            crate::dbglog!(
                "hook kb down vk=0x{vk:02X} scan=0x{:X} ext={} injected={} lower={} os_t={} mods=0x{:X} capturing={}",
                info.scanCode,
                info.flags.contains(LLKHF_EXTENDED),
                info.flags.contains(LLKHF_INJECTED),
                info.flags.contains(LLKHF_LOWER_IL_INJECTED),
                info.time,
                mods_bits(mods),
                st.capturing
            );
        }
        if st.capturing {
            // Esc cancels capture and is never bindable. Modifiers are
            // recorded and skipped so Ctrl+A captures Ctrl+A.
            if vk == 0x1B {
                post_capture(&st, CAP_CANCEL, 0, Modifiers::default());
            } else if modifier_flag(vk).is_none() {
                post_capture(&st, CAP_KEY, vk as usize, mods);
            }
            return;
        }
        trigger_down(&mut st, CAP_KEY, vk as usize, mods, info.vkCode as u16);
    } else {
        if crate::debuglog::enabled() && bound_key(&st, CAP_KEY, vk as usize) {
            crate::dbglog!(
                "hook kb up vk=0x{vk:02X} scan=0x{:X} ext={} injected={} os_t={}",
                info.scanCode,
                info.flags.contains(LLKHF_EXTENDED),
                info.flags.contains(LLKHF_INJECTED),
                info.time
            );
        }
        // Key-up is ignored while capturing (capture is down-only), but the
        // release trackers still have to be cleared: capture can start in the
        // middle of a hold.
        if !st.capturing {
            trigger_up(&mut st, CAP_KEY, vk as usize);
        }
    }
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

fn ms_event(msg: u32, lparam: LPARAM) {
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
    let Some(btn) = down_btn.or(up_btn) else {
        return;
    };
    let code = btn.id() as usize;
    let raw_vk = mouse_vk_code(btn);
    let mut st = state();

    if down_btn.is_some() {
        let mods = live_mods();
        if crate::debuglog::enabled() && (st.capturing || bound_key(&st, CAP_MOUSE, code)) {
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            crate::dbglog!(
                "hook ms down btn={:?} data=0x{:X} injected={} lower={} os_t={} mods=0x{:X} capturing={}",
                btn,
                info.mouseData,
                info.flags & LLMHF_INJECTED != 0,
                info.flags & LLMHF_LOWER_IL_INJECTED != 0,
                info.time,
                mods_bits(mods),
                st.capturing
            );
        }
        if st.capturing {
            post_capture(&st, CAP_MOUSE, code, mods);
            return;
        }
        trigger_down(&mut st, CAP_MOUSE, code, mods, raw_vk);
    } else {
        if crate::debuglog::enabled() && bound_key(&st, CAP_MOUSE, code) {
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            crate::dbglog!(
                "hook ms up btn={:?} data=0x{:X} injected={} os_t={}",
                btn,
                info.mouseData,
                info.flags & LLMHF_INJECTED != 0,
                info.time
            );
        }
        if !st.capturing {
            trigger_up(&mut st, CAP_MOUSE, code);
        }
    }
}

unsafe extern "system" fn kb_proc(ncode: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if ncode >= 0 {
        kb_event(wparam.0 as u32, lparam);
    }
    // Pass-through: never swallow, the key keeps working everywhere.
    CallNextHookEx(None, ncode, wparam, lparam)
}

unsafe extern "system" fn ms_proc(ncode: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if ncode >= 0 {
        ms_event(wparam.0 as u32, lparam);
    }
    // Pass-through: never swallow, the button keeps working everywhere.
    CallNextHookEx(None, ncode, wparam, lparam)
}

/// Install or drop the hooks to match the current config. Runs only on the
/// hook thread, which is also the thread that owns the hook handles.
fn sync_hooks(hinstance: HINSTANCE, kb: &mut Option<HHOOK>, ms: &mut Option<HHOOK>, refresh: bool) {
    if !state().wanted() {
        if let Some(h) = kb.take() {
            unsafe {
                let _ = UnhookWindowsHookEx(h);
            }
        }
        if let Some(h) = ms.take() {
            unsafe {
                let _ = UnhookWindowsHookEx(h);
            }
        }
        return;
    }
    if refresh {
        // There is no way to query a low-level hook, so replace it. If the
        // handle is already invalid Windows removed the hook behind our back.
        let mut gone = false;
        if let Some(h) = kb.take() {
            if unsafe { UnhookWindowsHookEx(h) }.is_err() {
                gone = true;
            }
        }
        if let Some(h) = ms.take() {
            if unsafe { UnhookWindowsHookEx(h) }.is_err() {
                gone = true;
            }
        }
        if gone {
            crate::dbglog!("hotkey hook had been removed by windows, reinstalled");
        }
    }
    if kb.is_none() {
        *kb = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), Some(hinstance), 0) }.ok();
        if kb.is_none() {
            crate::dbglog!("hotkey keyboard hook install failed");
        }
    }
    if ms.is_none() {
        *ms = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(ms_proc), Some(hinstance), 0) }.ok();
        if ms.is_none() {
            crate::dbglog!("hotkey mouse hook install failed");
        }
    }
}

fn hook_thread() {
    let module = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
    let hinstance = HINSTANCE(module.0);
    let mut kb: Option<HHOOK> = None;
    let mut ms: Option<HHOOK> = None;

    state().thread_id = unsafe { GetCurrentThreadId() };
    sync_hooks(hinstance, &mut kb, &mut ms, false);
    crate::dbglog!(
        "hotkey hooks installed kb={} ms={}",
        kb.is_some(),
        ms.is_some()
    );

    let mut msg = MSG::default();
    unsafe {
        // Create the thread message queue before SetTimer (thread timers need
        // one) and so PostThreadMessage from the UI thread always lands.
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        let _ = SetTimer(None, TIMER_HOOK_CHECK, HOOK_REFRESH_MS, None);
    }
    loop {
        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if ret.0 <= 0 {
            break;
        }
        // Thread messages (PostThreadMessage / thread timers) have no window.
        if msg.hwnd.is_invalid() {
            if msg.message == WM_HOTKEY_SYNC {
                sync_hooks(hinstance, &mut kb, &mut ms, false);
            } else if msg.message == WM_TIMER && msg.wParam.0 == TIMER_HOOK_CHECK {
                sync_hooks(hinstance, &mut kb, &mut ms, true);
            }
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
    }

    unsafe {
        let _ = KillTimer(None, TIMER_HOOK_CHECK);
        if let Some(h) = kb.take() {
            let _ = UnhookWindowsHookEx(h);
        }
        if let Some(h) = ms.take() {
            let _ = UnhookWindowsHookEx(h);
        }
    }
    state().thread_id = 0;
    crate::dbglog!("hotkey thread stopped");
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
    fn numpad_scancodes_normalize_across_numlock() {
        // NumLock off: Numpad0 arrives as VK_INSERT with the numpad scancode.
        assert_eq!(normalize_numpad(0x2D, 0x52, false), 0x60);
        assert_eq!(normalize_numpad(0x24, 0x47, false), 0x67);
        assert_eq!(normalize_numpad(0x25, 0x4B, false), 0x64);
        assert_eq!(normalize_numpad(0x2E, 0x53, false), 0x6E);
        // NumLock on still reports VK_NUMPADx; the mapping is stable.
        assert_eq!(normalize_numpad(0x60, 0x52, false), 0x60);
        assert_eq!(normalize_numpad(0x65, 0x4C, false), 0x65);
        // Dedicated keys use extended scancodes and keep their own VK.
        assert_eq!(normalize_numpad(0x2D, 0x52, true), 0x2D);
        assert_eq!(normalize_numpad(0x24, 0x47, true), 0x24);
        assert_eq!(normalize_numpad(0x41, 0x1E, false), 0x41);
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

    #[test]
    fn retrigger_guard_blocks_chatter_only() {
        let t0 = Instant::now();
        assert!(guard_allows(None, t0, 40));
        // First press accepted, chatter 5 ms later rejected.
        assert!(!guard_allows(
            Some(t0),
            t0 + std::time::Duration::from_millis(5),
            40
        ));
        // A normal second click 200 ms later is accepted.
        assert!(guard_allows(
            Some(t0),
            t0 + std::time::Duration::from_millis(200),
            40
        ));
        // 0 disables the guard entirely.
        assert!(guard_allows(Some(t0), t0, 0));
    }

    #[test]
    fn stale_press_is_cleared_only_when_the_key_is_up() {
        let now = Instant::now();
        // Nothing tracked: nothing to recover.
        assert!(!is_stale(None, false));
        assert!(!is_stale(None, true));
        // Tracked and still physically held (auto-repeat): keep the hold.
        assert!(!is_stale(Some(now), true));
        // Tracked but the OS says the key is up: the release was lost.
        assert!(is_stale(Some(now), false));
    }
}
