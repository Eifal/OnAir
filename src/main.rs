#![windows_subsystem = "windows"]
#![allow(static_mut_refs)]

mod config;
mod hotkey;
mod render;
mod tray;
mod vm;
mod window;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

use config::{AppConfig, MouseBtn, ToggleKey, TriggerMode};
use vm::{MicState, VmClient};
use window::{apply_lock, apply_pos, apply_topmost, clamp_position, nudge_into_view, to_wide, window_rect};

/// Keybind capture window: Esc cancels, opening the tray menu cancels.
const CAPTURE_TIMEOUT_MS: u64 = 15_000;

struct AppState {
    cfg: AppConfig,
    mic: MicState,
    vm: VmClient,
    hwnd: HWND,
    taskbar_msg: u32,
    tick: u32,
    tray_icon: Option<HICON>,
    // False until NIM_ADD has run. The first poll happens before the tray
    // exists, so refresh_tray() must not send NIM_MODIFY yet.
    tray_ready: bool,
    capture_until: Option<std::time::Instant>,
    // Hold-to-talk tracking: true between a matched key-down and key-up.
    hold_held: bool,
    // Display state saved on key-down (true=ON/green, false=OFF/red).
    hold_saved_on: Option<bool>,
    hold_down_at: Option<std::time::Instant>,
}

static mut APP: Option<AppState> = None;

// True between WM_ENTERSIZEMOVE and WM_EXITSIZEMOVE: the user is dragging
// the overlay in a modal move loop. The only legitimate non-self geometry
// change; everything else foreign gets rejected (see WM_WINDOWPOSCHANGING).
static mut IN_SIZE_MOVE: bool = false;

fn app_mut() -> &'static mut AppState {
    unsafe { APP.as_mut().expect("APP not initialized") }
}

/// Adopt the live window position into cfg (memory only, no file write).
/// Must be called on WM_MOVE and before handling menu commands: some moves
/// (Aero Snap, Win+arrow, display changes) bypass WM_EXITSIZEMOVE, leaving a
/// stale saved position that would otherwise teleport the window on the next
/// Size/Reload (apply_pos with the stale coords).
/// None-safe: window messages can arrive during CreateWindowExW,
/// before APP is initialized.
fn sync_pos_from_window() {
    let hwnd = unsafe { APP.as_ref().map(|a| a.hwnd).unwrap_or_default() };
    if hwnd.is_invalid() {
        return;
    }
    if let Some((x, y)) = window_rect(hwnd) {
        if let Some(a) = unsafe { APP.as_mut() } {
            if a.cfg.x != x || a.cfg.y != y {
                a.cfg.x = x;
                a.cfg.y = y;
            }
        }
    }
}

fn redraw_locked() {
    // Redraw overlay from current cfg + mic state.
    let (hwnd, size, opacity, locked, bg, dot, style, bg_op) = {
        let a = app_mut();
        (
            a.hwnd,
            a.cfg.size,
            a.cfg.opacity,
            a.cfg.locked,
            a.cfg.color_bg,
            a.cfg.dot_color(&a.mic),
            a.cfg.visual_style,
            a.cfg.bg_opacity,
        )
    };
    let pixels = render::draw_overlay(size, bg, dot, !locked, style, bg_op);
    render::update_layered_window(hwnd, &pixels, size, opacity);
}

fn refresh_tray() {
    if !app_mut().tray_ready {
        return;
    }
    let (hwnd, cfg_clone, mic_clone, bg) = {
        let a = app_mut();
        (a.hwnd, a.cfg.clone(), a.mic.clone(), a.cfg.color_bg)
    };
    let dot = cfg_clone.dot_color(&mic_clone);
    let new_icon = render::make_tray_icon(dot, bg);
    let tip = tray::tooltip_text(&cfg_clone, &mic_clone);
    // Swap icons (destroy old after modify to avoid flicker using null)
    let old = app_mut().tray_icon.replace(new_icon.unwrap_or_default());
    let cur = app_mut().tray_icon.unwrap_or_default();
    let cur_opt = if cur.is_invalid() { None } else { Some(cur) };
    tray::modify_tray(hwnd, cur_opt, &tip);
    if let Some(o) = old {
        if !o.is_invalid() && o != cur {
            unsafe { let _ = DestroyIcon(o); }
        }
    }
}

fn do_poll_and_update() {
    let (param, invert) = {
        let a = app_mut();
        (a.cfg.vm_param.clone(), a.cfg.invert)
    };
    let new_state = app_mut().vm.poll(&param, invert);
    let changed = {
        let a = app_mut();
        if a.mic != new_state {
            a.mic = new_state;
            true
        } else {
            false
        }
    };
    if changed {
        redraw_locked();
        refresh_tray();
    }
}

/// Flip the configured Voicemeeter button via the Remote API, then poll
/// immediately so the overlay reflects the result. Silent on any failure.
fn do_toggle() {
    let param = app_mut().cfg.vm_param.clone();
    app_mut().vm.toggle_param(&param);
    do_poll_and_update();
}

/// Raw value that renders as display ON/OFF (accounts for `invert`,
/// which is display-only).
fn raw_for_display(display_on: bool, invert: bool) -> f32 {
    let on_raw = if invert { 0.0 } else { 1.0 };
    if display_on { on_raw } else { 1.0 - on_raw }
}

/// Key-down in Hold/Both mode: save the current display state and force
/// mic ON (push-to-talk). Ignores repeats (already held) and Unknown
/// state. Silent on any VM failure (tracking is still set so the
/// matching key-up is consumed cleanly).
fn do_hold_down() {
    if app_mut().hold_held {
        return; // auto-repeat guard (backup for the hook-side filter)
    }
    if app_mut().cfg.toggle_key.is_none() {
        return;
    }
    let saved_on = match app_mut().mic {
        MicState::On => Some(true),
        MicState::Off => Some(false),
        MicState::Unknown(_) => None,
    };
    let Some(saved) = saved_on else {
        return; // Voicemeeter unreachable: don't latch a hold
    };
    {
        let a = app_mut();
        a.hold_held = true;
        a.hold_saved_on = Some(saved);
        a.hold_down_at = Some(std::time::Instant::now());
    }
    let (param, invert) = {
        let a = app_mut();
        (a.cfg.vm_param.clone(), a.cfg.invert)
    };
    app_mut().vm.set_raw(&param, raw_for_display(true, invert));
    do_poll_and_update();
}

/// Key-up in Hold mode: restore the state saved on key-down.
fn do_hold_up_restore() {
    if !app_mut().hold_held {
        return;
    }
    let (param, invert, saved) = {
        let a = app_mut();
        let s = a.hold_saved_on;
        a.hold_held = false;
        a.hold_saved_on = None;
        a.hold_down_at = None;
        (a.cfg.vm_param.clone(), a.cfg.invert, s)
    };
    if let Some(saved_on) = saved {
        app_mut().vm.set_raw(&param, raw_for_display(saved_on, invert));
        do_poll_and_update();
    }
}

/// Key-up in Both mode: tap (< hold_tap_ms) toggles from the saved state,
/// longer hold restores it (momentary).
fn do_hold_up_both() {
    if !app_mut().hold_held {
        return;
    }
    let (param, invert, saved, thresh) = {
        let a = app_mut();
        let dur_ms = a
            .hold_down_at
            .map(|t| std::time::Instant::now().duration_since(t).as_millis() as u64)
            .unwrap_or(u64::MAX);
        let s = a.hold_saved_on;
        let th = a.cfg.hold_tap_ms as u64;
        a.hold_held = false;
        a.hold_saved_on = None;
        a.hold_down_at = None;
        (a.cfg.vm_param.clone(), a.cfg.invert, s, (dur_ms, th))
    };
    let (dur_ms, thresh_ms) = thresh;
    if let Some(saved_on) = saved {
        let target_on = if dur_ms < thresh_ms {
            !saved_on // tap: toggle
        } else {
            saved_on // hold: momentary, restore
        };
        app_mut()
            .vm
            .set_raw(&param, raw_for_display(target_on, invert));
        do_poll_and_update();
    }
}

/// Clear hold tracking without touching Voicemeeter (used on config
/// reload where the param itself may have changed).
fn reset_hold_tracking() {
    let a = app_mut();
    a.hold_held = false;
    a.hold_saved_on = None;
    a.hold_down_at = None;
}

/// If a hold is active, restore the saved state first (used before the
/// bind/mode changes so the mic never sticks ON).
fn restore_hold_if_held() {
    if !app_mut().hold_held {
        return;
    }
    // Both and Hold restore identically here: the mode is about to change,
    // so treat the release as a long hold (back to saved).
    do_hold_up_restore();
}

/// Check an incoming WM_KEY_DOWN/UP against the current bind (guards
/// against stale queued messages after the bind changed).
fn key_event_matches(kind: usize, code: usize) -> bool {
    let bind = app_mut().cfg.toggle_key;
    match bind {
        ToggleKey::None => false,
        ToggleKey::Key(vk) => kind == hotkey::CAP_KEY && code == vk as usize,
        ToggleKey::Mouse(b) => kind == hotkey::CAP_MOUSE && code == b.id() as usize,
    }
}

fn on_key_down(kind: usize, code: usize) {
    if !key_event_matches(kind, code) {
        return;
    }
    match app_mut().cfg.trigger_mode {
        TriggerMode::Toggle => do_toggle(),
        TriggerMode::Hold | TriggerMode::Both => do_hold_down(),
    }
}

fn on_key_up(kind: usize, code: usize) {
    if !key_event_matches(kind, code) {
        // Stale up for a previous bind: drop any latch so the next
        // press starts clean. No VM write (bind already changed).
        reset_hold_tracking();
        return;
    }
    match app_mut().cfg.trigger_mode {
        TriggerMode::Toggle => {}
        TriggerMode::Hold => do_hold_up_restore(),
        TriggerMode::Both => do_hold_up_both(),
    }
}

fn begin_capture() {
    hotkey::set_capturing(true);
    app_mut().capture_until =
        Some(std::time::Instant::now() + std::time::Duration::from_millis(CAPTURE_TIMEOUT_MS));
    let hwnd = app_mut().hwnd;
    let cur = app_mut().tray_icon.unwrap_or_default();
    tray::modify_tray(
        hwnd,
        if cur.is_invalid() { None } else { Some(cur) },
        "Press a key or mouse button… (Esc cancels)",
    );
}

fn cancel_capture() {
    hotkey::set_capturing(false);
    if let Some(a) = unsafe { APP.as_mut() } {
        a.capture_until = None;
    }
    refresh_tray();
}

fn apply_all_after_config_change() {
    // Called after reload or menu change affecting geometry/styles.
    // The caller must release a held PTT before swapping the parameter.
    // This reset is only a safety net and intentionally performs no VM
    // write, because the old parameter is no longer available here.
    reset_hold_tracking();
    let (hwnd, x, y, size, locked, topmost, poll_ms) = {
        let a = app_mut();
        (a.hwnd, a.cfg.x, a.cfg.y, a.cfg.size, a.cfg.locked, a.cfg.always_on_top, a.cfg.poll_ms)
    };
    let (cx, cy) = clamp_position(x, y, size);
    {
        let a = app_mut();
        a.cfg.x = cx;
        a.cfg.y = cy;
    }
    apply_pos(hwnd, cx, cy, size);
    apply_lock(hwnd, locked);
    apply_topmost(hwnd, topmost);
    // Keep the hook shadow bind in sync (bind may have come from the file).
    hotkey::set_bind(app_mut().cfg.toggle_key);
    unsafe {
        let _ = KillTimer(Some(hwnd), window::TIMER_POLL);
        let _ = SetTimer(Some(hwnd), window::TIMER_POLL, poll_ms, None);
    }
    // Force fresh poll so green/red reacts immediately to param/invert change.
    do_poll_and_update();
    redraw_locked();
    refresh_tray();
    let _ = app_mut().cfg.save();
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // TaskbarCreated is dynamic; compare if APP initialized.
    let taskbar = unsafe { APP.as_ref().map(|a| a.taskbar_msg).unwrap_or(0) };
    if msg == taskbar && taskbar != 0 {
        // Explorer restarted: re-add icon
        let (cfg_c, mic_c, bg) = {
            let a = app_mut();
            (a.cfg.clone(), a.mic.clone(), a.cfg.color_bg)
        };
        let dot = cfg_c.dot_color(&mic_c);
        let icon = render::make_tray_icon(dot, bg);
        let tip = tray::tooltip_text(&cfg_c, &mic_c);
        if let Some(ic) = icon {
            // replace stored icon
            let old = app_mut().tray_icon.replace(ic);
            if let Some(o) = old {
                if !o.is_invalid() { let _ = DestroyIcon(o); }
            }
        }
        let cur = app_mut().tray_icon.unwrap_or_default();
        tray::add_tray(hwnd, if cur.is_invalid() { None } else { Some(cur) }, &tip);
        app_mut().tray_ready = true;
        return LRESULT(0);
    }

    match msg {
        WM_NCHITTEST => {
            let locked = unsafe { APP.as_ref().map(|a| a.cfg.locked).unwrap_or(true) };
            if locked {
                LRESULT(HTTRANSPARENT as isize)
            } else {
                LRESULT(HTCAPTION as isize)
            }
        }
        WM_WINDOWPOSCHANGING => {
            // GUARD: the overlay may only change geometry via a user drag
            // (modal loop) or our own flagged calls (window::geo_self).
            // Anything else (other processes, snap commits outside drags,
            // focus side-effects) is rejected by forcing NOMOVE|NOSIZE.
            // Show/hide/z-order bits are never touched, and requests already
            // matching our geometry pass through untouched (e.g. redraw's
            // UpdateLayeredWindow).
            if unsafe { APP.is_none() } {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            let self_init = window::geo_self();
            let drag = unsafe { IN_SIZE_MOVE };
            unsafe {
                let wp = &mut *(lparam.0 as *mut WINDOWPOS);
                if !self_init && !drag {
                    if let Some(a) = APP.as_ref() {
                        let want_move = (wp.flags.0 & SWP_NOMOVE.0) == 0
                            && (wp.x != a.cfg.x || wp.y != a.cfg.y);
                        let want_size = (wp.flags.0 & SWP_NOSIZE.0) == 0
                            && (wp.cx != a.cfg.size as i32 || wp.cy != a.cfg.size as i32);
                        if want_move || want_size {
                            wp.flags |= SWP_NOMOVE | SWP_NOSIZE;
                        }
                    }
                } else if !self_init && drag {
                    // During a user drag, never let anyone resize us
                    // (e.g. Aero Snap to half-screen); position follows
                    // the user's hand.
                    if let Some(a) = APP.as_ref() {
                        if (wp.flags.0 & SWP_NOSIZE.0) == 0
                            && (wp.cx != a.cfg.size as i32 || wp.cy != a.cfg.size as i32)
                        {
                            wp.flags |= SWP_NOSIZE;
                        }
                    }
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_ENTERSIZEMOVE => {
            unsafe {
                IN_SIZE_MOVE = true;
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_EXITSIZEMOVE => {
            unsafe {
                IN_SIZE_MOVE = false;
            }
            if let Some((x, y)) = window_rect(hwnd) {
                let save = {
                    let a = app_mut();
                    a.cfg.x = x;
                    a.cfg.y = y;
                    a.cfg.clone()
                };
                let _ = save.save();
            }
            LRESULT(0)
        }
        WM_MOVE => {
            // Live-sync position so cfg never goes stale (Aero Snap,
            // Win+arrows, etc. don't send WM_EXITSIZEMOVE). No file write
            // here; persistence happens on EXITSIZEMOVE / menu saves / exit.
            sync_pos_from_window();
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_DPICHANGED => {
            // May arrive during window creation (before APP exists).
            if unsafe { APP.is_none() } {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            // Keep physical pixels per spec: re-assert the live position
            // with cfg.size instead of Windows' suggested scaled rect.
            sync_pos_from_window();
            let (x, y, size) = {
                let a = app_mut();
                (a.cfg.x, a.cfg.y, a.cfg.size)
            };
            apply_pos(hwnd, x, y, size);
            redraw_locked();
            let _ = app_mut().cfg.save();
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            let (x, y, size) = {
                let a = app_mut();
                (a.cfg.x, a.cfg.y, a.cfg.size)
            };
            let (cx, cy) = clamp_position(x, y, size);
            if (cx, cy) != (x, y) {
                {
                    let a = app_mut();
                    a.cfg.x = cx;
                    a.cfg.y = cy;
                }
                apply_pos(hwnd, cx, cy, size);
                let _ = app_mut().cfg.save();
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == window::TIMER_POLL {
                do_poll_and_update();
                // Keybind capture timeout.
                if hotkey::is_capturing() {
                    let expired = app_mut()
                        .capture_until
                        .map(|d| std::time::Instant::now() >= d)
                        .unwrap_or(true);
                    if expired {
                        cancel_capture();
                    }
                }
                // Re-assert topmost ~every 2s
                let (topmost, poll_ms) = {
                    let a = app_mut();
                    a.tick = a.tick.wrapping_add(1);
                    (a.cfg.always_on_top, a.cfg.poll_ms)
                };
                if topmost {
                    let ticks_needed = ((2000u32 / poll_ms.max(1)).max(1)) as u32;
                    let tick = unsafe { APP.as_ref().map(|a| a.tick).unwrap_or(0) };
                    if tick >= ticks_needed {
                        app_mut().tick = 0;
                        let h1 = unsafe {
                            APP.as_ref().map(|a| a.hwnd).unwrap_or_default()
                        };
                        apply_topmost(h1, true);
                    }
                }
            }
            LRESULT(0)
        }
        m if m == window::WM_TRAY => {
            // A tray click while capturing cancels capture instead of
            // opening the menu.
            if hotkey::is_capturing() {
                cancel_capture();
                return LRESULT(0);
            }
            let mouse = (lparam.0 as u32) & 0xFFFF;
            // Legacy: 514 WM_LBUTTONUP, 517 WM_RBUTTONUP.
            // NOTIFYICON_VERSION_4: 123 WM_CONTEXTMENU (right),
            // 1024 NIN_SELECT (left). Handle both so the menu opens
            // reliably on all Windows 10/11 builds.
            if mouse == WM_LBUTTONUP
                || mouse == WM_RBUTTONUP
                || mouse == 123
                || mouse == 1024
            {
                // Reentrancy guard: some systems deliver two notifications
                // for one click; never stack two modal menus.
                static mut IN_MENU: bool = false;
                if !unsafe { IN_MENU } {
                    unsafe { IN_MENU = true; }
                    let cfg_c = app_mut().cfg.clone();
                    tray::show_menu(hwnd, &cfg_c);
                    unsafe { IN_MENU = false; }
                }
            }
            LRESULT(0)
        }
        m if m == window::WM_TOGGLE_REQ => {
            // Legacy single-shot toggle message (hooks now send
            // WM_KEY_DOWN/UP instead; kept so any queued message still
            // behaves like a press in Toggle mode).
            if !app_mut().cfg.toggle_key.is_none()
                && matches!(app_mut().cfg.trigger_mode, TriggerMode::Toggle)
            {
                do_toggle();
            }
            LRESULT(0)
        }
        m if m == window::WM_KEY_DOWN => {
            // Global bind pressed (event already passed through to the
            // system by the hook proc).
            on_key_down(wparam.0, lparam.0 as usize);
            LRESULT(0)
        }
        m if m == window::WM_KEY_UP => {
            // Global bind released.
            on_key_up(wparam.0, lparam.0 as usize);
            LRESULT(0)
        }
        m if m == window::WM_KEY_CAPTURED => {
            if hotkey::is_capturing() {
                let kind = wparam.0 as usize;
                let code = lparam.0 as usize;
                let key = match kind {
                    hotkey::CAP_KEY => ToggleKey::Key(code as u16),
                    hotkey::CAP_MOUSE => MouseBtn::from_id(code as u8)
                        .map(ToggleKey::Mouse)
                        .unwrap_or(ToggleKey::None),
                    _ => ToggleKey::None, // CAP_CANCEL (Esc)
                };
                hotkey::set_capturing(false);
                app_mut().capture_until = None;
                if !key.is_none() {
                    app_mut().cfg.toggle_key = key;
                    hotkey::set_bind(key);
                    let _ = app_mut().cfg.save();
                }
                refresh_tray();
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 as u32) & 0xFFFF;
            handle_command(hwnd, id);
            LRESULT(0)
        }
        WM_ENDSESSION => {
            // Windows is ending the session. Release a held PTT before the
            // process is terminated; WM_DESTROY may not be sent in time.
            if wparam.0 != 0 {
                restore_hold_if_held();
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // Safety net for any destruction path that did not go through
            // ID_QUIT (for example DefWindowProc handling WM_CLOSE).
            restore_hold_if_held();
            app_mut().vm.shutdown();
            unsafe {
                let _ = KillTimer(Some(hwnd), window::TIMER_POLL);
            }
            hotkey::shutdown();
            tray::remove_tray(hwnd);
            // VmClient::drop does Logout + FreeLibrary.
            // Destroy stored icon.
            if let Some(ic) = app_mut().tray_icon.take() {
                if !ic.is_invalid() {
                    unsafe { let _ = DestroyIcon(ic); }
                }
            }
            unsafe { PostQuitMessage(0); }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn handle_command(hwnd: HWND, id: u32) {
    use tray::*;
    // Adopt the live position first so no menu command can teleport
    // the window back to a stale saved position.
    sync_pos_from_window();
    if id == ID_QUIT {
        // Release a held PTT before destroying the window. Otherwise the
        // mic can stay ON after the app exits.
        restore_hold_if_held();
        let _ = app_mut().cfg.save();
        unsafe { let _ = DestroyWindow(hwnd); }
        return;
    }
    if id == ID_LOCK {
        let v = !app_mut().cfg.locked;
        app_mut().cfg.locked = v;
        apply_lock(hwnd, v);
        redraw_locked();
        let _ = app_mut().cfg.save();
        return;
    }
    if id == ID_TOPMOST {
        let v = !app_mut().cfg.always_on_top;
        app_mut().cfg.always_on_top = v;
        apply_topmost(hwnd, v);
        let _ = app_mut().cfg.save();
        return;
    }
    if id == ID_INVERT {
        // Release an active PTT before changing how the raw value is
        // interpreted; otherwise key-up could write the saved state using
        // the new invert flag.
        restore_hold_if_held();
        let v = !app_mut().cfg.invert;
        app_mut().cfg.invert = v;
        let _ = app_mut().cfg.save();
        do_poll_and_update();
        redraw_locked();
        refresh_tray();
        return;
    }
    if id == ID_OPEN_CFG {
        tray::open_config_file();
        return;
    }
    if id == ID_RELOAD_CFG {
        // Restore against the old parameter before replacing the config;
        // after the swap we no longer know which parameter was held.
        restore_hold_if_held();
        let cfg = AppConfig::load();
        app_mut().cfg = cfg;
        apply_all_after_config_change();
        return;
    }
    if id == ID_TOGGLE_SET {
        // Don't let a hold stick ON across re-binding.
        restore_hold_if_held();
        begin_capture();
        return;
    }
    if id == ID_TOGGLE_CLEAR {
        restore_hold_if_held();
        hotkey::set_capturing(false);
        app_mut().capture_until = None;
        app_mut().cfg.toggle_key = ToggleKey::None;
        hotkey::set_bind(ToggleKey::None);
        let _ = app_mut().cfg.save();
        refresh_tray();
        return;
    }
    if id == ID_TRIGGER_TOGGLE || id == ID_TRIGGER_HOLD || id == ID_TRIGGER_BOTH {
        restore_hold_if_held();
        app_mut().cfg.trigger_mode = match id {
            ID_TRIGGER_HOLD => TriggerMode::Hold,
            ID_TRIGGER_BOTH => TriggerMode::Both,
            _ => TriggerMode::Toggle,
        };
        let _ = app_mut().cfg.save();
        refresh_tray();
        return;
    }
    if (ID_HOLDMS_BASE..ID_HOLDMS_BASE + 4).contains(&id) {
        let idx = (id - ID_HOLDMS_BASE) as usize;
        if let Some(&ms) = HOLD_TAPS.get(idx) {
            app_mut().cfg.hold_tap_ms = ms;
            let _ = app_mut().cfg.save();
            refresh_tray();
        }
        return;
    }
    if (ID_SIZE_BASE..ID_SIZE_BASE + 6).contains(&id) {
        let idx = (id - ID_SIZE_BASE) as usize;
        if let Some(&s) = SIZES.get(idx) {
            app_mut().cfg.size = s;
            // Keep position; only nudge minimally to stay on-screen.
            // (Never reset to (40,40) here — that teleport was the bug.)
            let (x, y) = { let a = app_mut(); (a.cfg.x, a.cfg.y) };
            let (cx, cy) = nudge_into_view(x, y, s);
            app_mut().cfg.x = cx;
            app_mut().cfg.y = cy;
            apply_pos(hwnd, cx, cy, s);
            redraw_locked();
            let _ = app_mut().cfg.save();
        }
        return;
    }
    if (ID_OPACITY_BASE..ID_OPACITY_BASE + 9).contains(&id) {
        let idx = (id - ID_OPACITY_BASE) as usize;
        if let Some(&o) = OPACITIES.get(idx) {
            app_mut().cfg.opacity = o;
            redraw_locked();
            let _ = app_mut().cfg.save();
        }
        return;
    }
    if (ID_VISUAL_BASE..ID_VISUAL_BASE + 4).contains(&id) {
        let idx = (id - ID_VISUAL_BASE) as usize;
        if let Some(&(style, _)) = VISUALS.get(idx) {
            app_mut().cfg.visual_style = style;
            redraw_locked();
            let _ = app_mut().cfg.save();
            refresh_tray();
        }
        return;
    }
    if (ID_BG_BASE..ID_BG_BASE + 6).contains(&id) {
        let idx = (id - ID_BG_BASE) as usize;
        if let Some(&o) = BG_OPACITIES.get(idx) {
            app_mut().cfg.bg_opacity = o;
            redraw_locked();
            let _ = app_mut().cfg.save();
        }
        return;
    }
    if (ID_STRIP_BASE..ID_STRIP_BASE + 8).contains(&id) {
        // Restore against the currently held parameter before switching
        // to a different strip.
        restore_hold_if_held();
        let strip1 = id - ID_STRIP_BASE + 1; // 1-based
        let btn = tray::parse_strip_button(&app_mut().cfg.vm_param)
            .map(|(_, b)| b)
            .unwrap_or_else(|| "B2".to_string());
        app_mut().cfg.vm_param = format!("Strip[{}].{}", strip1 - 1, btn);
        let _ = app_mut().cfg.save();
        do_poll_and_update();
        refresh_tray();
        return;
    }
    if (ID_BTN_BASE..ID_BTN_BASE + 9).contains(&id) {
        // Restore against the currently held button before switching.
        restore_hold_if_held();
        let idx = (id - ID_BTN_BASE) as usize;
        if let Some(&b) = BUTTONS.get(idx) {
            let strip0 = tray::parse_strip_button(&app_mut().cfg.vm_param)
                .map(|(s, _)| s - 1)
                .unwrap_or(0);
            app_mut().cfg.vm_param = format!("Strip[{}].{}", strip0, b);
            let _ = app_mut().cfg.save();
            do_poll_and_update();
            refresh_tray();
        }
        return;
    }
}

fn main() {
    unsafe {
        // DPI: physical pixels
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        // Single instance
        let mname = to_wide(window::MUTEX_NAME);
        let mutex = CreateMutexW(None, false, PCWSTR(mname.as_ptr()));
        if mutex.is_err() {
            return;
        }
        if windows::Win32::Foundation::GetLastError() == ERROR_ALREADY_EXISTS {
            return; // exit silently
        }

        // Config
        let mut cfg = AppConfig::load();
        let (cx, cy) = clamp_position(cfg.x, cfg.y, cfg.size);
        cfg.x = cx;
        cfg.y = cy;

        let hmodule = GetModuleHandleW(None).unwrap_or_default();
        // HMODULE and HINSTANCE share layout; convert via raw value.
        let hinstance = HINSTANCE(hmodule.0);

        let cls_w = to_wide(window::CLASS_NAME);
        let wnd_class = WNDCLASSW {
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: HICON::default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: PCWSTR(cls_w.as_ptr()),
        };
        if RegisterClassW(&wnd_class) == 0 {
            return;
        }

        let mut ex_style = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        if cfg.always_on_top {
            ex_style |= WS_EX_TOPMOST;
        }
        if cfg.locked {
            ex_style |= WS_EX_TRANSPARENT;
        }

        let title = to_wide("VM Mic Indicator");
        let hwnd = CreateWindowExW(
            ex_style,
            PCWSTR(cls_w.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            cfg.x,
            cfg.y,
            cfg.size as i32,
            cfg.size as i32,
            None,
            None,
            Some(hinstance),
            None,
        );
        if hwnd.is_err() {
            return;
        }
        let hwnd = hwnd.unwrap();

        let taskbar_msg = tray::taskbar_created_msg();
        let vm = VmClient::new();
        // Initial unknown until first poll; do one poll now for fast startup paint.
        APP = Some(AppState {
            cfg,
            mic: MicState::Unknown("init".to_string()),
            vm,
            hwnd,
            taskbar_msg,
            tick: 0,
            tray_icon: None,
            tray_ready: false,
            capture_until: None,
            hold_held: false,
            hold_saved_on: None,
            hold_down_at: None,
        });

        // First poll + paint before showing (avoids gray flash when VM running? still gray on miss)
        do_poll_and_update();
        // Position/size/styles already set by CreateWindowEx; enforce via helpers.
        {
            let a = app_mut();
            apply_pos(a.hwnd, a.cfg.x, a.cfg.y, a.cfg.size);
            apply_lock(a.hwnd, a.cfg.locked);
            apply_topmost(a.hwnd, a.cfg.always_on_top);
        }
        redraw_locked();

        // Tray. refresh_tray() is disabled until NIM_ADD has run; install
        // the initial icon here, then enable updates.
        {
            let a = app_mut();
            let dot = a.cfg.dot_color(&a.mic);
            let icon = render::make_tray_icon(dot, a.cfg.color_bg);
            let tip = tray::tooltip_text(&a.cfg, &a.mic);
            if let Some(ic) = icon {
                if let Some(old) = a.tray_icon.replace(ic) {
                    if !old.is_invalid() {
                        let _ = DestroyIcon(old);
                    }
                }
            }
            let cur = a.tray_icon.unwrap_or_default();
            tray::add_tray(hwnd, if cur.is_invalid() { None } else { Some(cur) }, &tip);
            a.tray_ready = true;
        }

        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        // NOTE: no SetFocus / SetActiveWindow anywhere. All SetWindowPos use NOACTIVATE.

        // Global keybind hooks (low-level keyboard + mouse).
        // Installed just before the message loop so hook events can only
        // arrive once we are pumping messages. Inactive if it fails.
        {
            let mic_bind = app_mut().cfg.toggle_key;
            hotkey::init(hwnd, hinstance, mic_bind);
        }

        let poll = app_mut().cfg.poll_ms;
        let _ = SetTimer(Some(hwnd), window::TIMER_POLL, poll, None);

        let mut msg = MSG::default();
        loop {
            let ret = GetMessageW(&mut msg, None, 0, 0);
            if ret.0 == 0 || ret.0 == -1 {
                break; // WM_QUIT or error
            }
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }

        // Save position on exit (in case drag-end save missed)
        if let Some(a) = APP.as_mut() {
            // Fallback for a WM_QUIT that arrived without WM_DESTROY.
            a.vm.shutdown();
            if let Some((x, y)) = window_rect(a.hwnd) {
                a.cfg.x = x;
                a.cfg.y = y;
            }
            let _ = a.cfg.save();
        }
    }
}
