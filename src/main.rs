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

use config::{AppConfig, MouseBtn, ToggleKey};
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
    capture_until: Option<std::time::Instant>,
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
    let (hwnd, size, opacity, locked, bg, dot) = {
        let a = app_mut();
        (
            a.hwnd,
            a.cfg.size,
            a.cfg.opacity,
            a.cfg.locked,
            a.cfg.color_bg,
            a.cfg.dot_color(&a.mic),
        )
    };
    let pixels = render::draw_overlay(size, bg, dot, !locked);
    render::update_layered_window(hwnd, &pixels, size, opacity);
}

fn refresh_tray() {
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
            // Global toggle keybind matched (event already passed through
            // to the system by the hook proc).
            if !app_mut().cfg.toggle_key.is_none() {
                do_toggle();
            }
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
        WM_DESTROY => {
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
        let cfg = AppConfig::load();
        app_mut().cfg = cfg;
        apply_all_after_config_change();
        return;
    }
    if id == ID_TOGGLE_SET {
        begin_capture();
        return;
    }
    if id == ID_TOGGLE_CLEAR {
        hotkey::set_capturing(false);
        app_mut().capture_until = None;
        app_mut().cfg.toggle_key = ToggleKey::None;
        hotkey::set_bind(ToggleKey::None);
        let _ = app_mut().cfg.save();
        refresh_tray();
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
    if (ID_STRIP_BASE..ID_STRIP_BASE + 8).contains(&id) {
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
            capture_until: None,
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

        // Tray
        {
            let a = app_mut();
            let dot = a.cfg.dot_color(&a.mic);
            let icon = render::make_tray_icon(dot, a.cfg.color_bg);
            let tip = tray::tooltip_text(&a.cfg, &a.mic);
            if let Some(ic) = icon {
                a.tray_icon = Some(ic);
            }
            let cur = a.tray_icon.unwrap_or_default();
            tray::add_tray(hwnd, if cur.is_invalid() { None } else { Some(cur) }, &tip);
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
            if let Some((x, y)) = window_rect(a.hwnd) {
                a.cfg.x = x;
                a.cfg.y = y;
            }
            let _ = a.cfg.save();
        }
    }
}
