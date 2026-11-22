#![windows_subsystem = "windows"]
#![allow(static_mut_refs)]

mod config;
mod glassmenu;
mod hotkey;
mod render;
mod tray;
mod vm;
mod wave;
mod window;

use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use config::{
    parse_macro_script, AppConfig, ConfigKey, MacroMode, MacroOp, MacroStep, MenuStyle, MouseBtn,
    ToggleKey, Trigger, TriggerMode, VisualStyle, VmMode,
};
use vm::{MicState, VmClient};
use window::{
    apply_lock, apply_pos, apply_topmost, clamp_position, nudge_into_view, to_wide, window_rect,
};

/// Keybind capture window: Esc cancels, opening the tray menu cancels.
const CAPTURE_TIMEOUT_MS: u64 = 15_000;

/// Values the overlay owns. It must never rewrite the whole config: comments,
/// unknown keys and settings-owned values belong to onair-settings.
const POSITION_KEYS: &[ConfigKey] = &[ConfigKey::X, ConfigKey::Y];
const OVERLAY_KEYS: &[ConfigKey] = &[
    ConfigKey::X,
    ConfigKey::Y,
    ConfigKey::Locked,
    ConfigKey::AlwaysOnTop,
    ConfigKey::OverlayVisible,
];

/// Per-macro state between key-down and key-up.
#[derive(Default)]
struct MacroRuntime {
    held: bool,
    /// Raw parameter values read on key-down, used when the macro has no
    /// release script (Hold/Both restore the previous values).
    saved: Vec<(String, f32)>,
    down_at: Option<std::time::Instant>,
}

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
    /// 0 = overlay hotkey, 1 + i = macro i.
    capture_target: usize,
    /// Parsed press/release scripts, index-aligned with cfg.macros.
    macro_steps: Vec<(Vec<MacroStep>, Vec<MacroStep>)>,
    macro_runtime: Vec<MacroRuntime>,
    // Hold-to-talk tracking: true between a matched key-down and key-up.
    hold_held: bool,
    // Display state saved on key-down (true=ON/green, false=OFF/red).
    hold_saved_on: Option<bool>,
    // Raw A/B states saved on key-down in switch mode.
    hold_saved_switch: Option<(bool, bool)>,
    hold_down_at: Option<std::time::Instant>,
    reload_msg: u32,
    capture_msg: u32,
    /// Reused pixel buffer for every overlay frame (no per-frame allocation).
    pixels: Vec<u32>,
    /// Reused DIB + memory DC, recreated only when the overlay size changes.
    surface: Option<render::Surface>,
    /// `Some` while the animation timer is running (wave style only).
    speaking: Option<wave::Speaking>,
    /// Input channels captured when the animation starts.
    anim_channels: Option<(u32, u32)>,
    /// Timestamp of the previous animation tick, for frame-rate independent
    /// phase and release timing.
    anim_last: Option<std::time::Instant>,
    anim_ms: u32,
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

/// Draw one frame. `speaking` is `(smoothed level, phase seconds)` for the
/// wave style; `None` draws the static/minimum-bar frame. The pixel buffer
/// and DIB are reused.
fn draw_frame(speaking: Option<(f32, f32)>) {
    let (hwnd, size, opacity, locked, bg, dot, style, bg_op, bars) = {
        let a = app_mut();
        (
            a.hwnd,
            a.cfg.size,
            a.cfg.opacity,
            a.cfg.locked,
            a.cfg.color_bg,
            vm::dot_color(&a.cfg, &a.mic),
            a.cfg.visual_style,
            a.cfg.bg_opacity,
            a.cfg.wave_bars,
        )
    };
    let needed = (size as usize) * (size as usize);
    let wave_view = if style == VisualStyle::Wave {
        let (level, phase) = speaking.unwrap_or((0.0, 0.0));
        Some(render::WaveView { level, phase, bars })
    } else {
        None
    };
    let a = app_mut();
    if a.pixels.len() != needed {
        a.pixels.resize(needed, 0);
    }
    render::draw_overlay_into(
        &mut a.pixels,
        size,
        bg,
        dot,
        !locked,
        style,
        bg_op,
        wave_view,
    );
    if a.surface.as_ref().map(|s| s.size()) != Some((size, size)) {
        a.surface = render::Surface::new(size, size);
    }
    if let Some(surface) = a.surface.as_mut() {
        surface.upload(hwnd, &a.pixels, opacity);
    }
}

fn redraw_locked() {
    draw_frame(None);
}

fn redraw_speaking() {
    let frame = app_mut().speaking.map(|s| (s.level, s.phase));
    draw_frame(frame);
}

/// Stop the animation timer and drop the speaking state. The caller decides
/// whether a final static frame is needed.
fn stop_anim() {
    let hwnd = app_mut().hwnd;
    unsafe {
        let _ = KillTimer(Some(hwnd), window::TIMER_ANIM);
    }
    let a = app_mut();
    a.speaking = None;
    a.anim_channels = None;
    a.anim_last = None;
    a.anim_ms = 0;
}

/// Input channels for the current `vm_param`. A manual `level_channel`
/// bypasses the edition/strip mapping. `None` means "show a static wave".
fn wave_channels() -> Option<(u32, u32)> {
    let (manual, strip) = {
        let a = app_mut();
        (a.cfg.level_channel, vm::strip_index(&a.cfg.vm_param))
    };
    if let Some(channel) = manual {
        return Some((channel, channel.saturating_add(1)));
    }
    let strip = strip?;
    let edition = app_mut().vm.edition()?;
    vm::strip_channels(strip, edition)
}

/// Read the louder of the two input channels and normalize it against the
/// current threshold. `None` when the level API is unavailable.
fn level_norm(channels: (u32, u32), level_type: u32, threshold_db: f32) -> Option<f32> {
    let amp = {
        let a = app_mut();
        let left = a.vm.get_level(level_type, channels.0);
        let right = a.vm.get_level(level_type, channels.1);
        match (left, right) {
            (None, None) => return None,
            (a, b) => a.unwrap_or(0.0).max(b.unwrap_or(0.0)),
        }
    };
    Some(wave::normalize_db(wave::amp_to_db(amp), threshold_db))
}

fn start_speaking(level: f32, channels: (u32, u32)) {
    let (hwnd, fps) = {
        let a = app_mut();
        (a.hwnd, a.cfg.anim_fps)
    };
    let interval = (1000 / fps.max(1)).max(10);
    {
        let a = app_mut();
        a.speaking = Some(wave::Speaking::new(level));
        a.anim_channels = Some(channels);
        a.anim_last = Some(std::time::Instant::now());
        a.anim_ms = interval;
    }
    unsafe {
        let _ = SetTimer(Some(hwnd), window::TIMER_ANIM, interval, None);
    }
    redraw_speaking();
}

/// Poll-tick work for the wave style. Reads the level only when the overlay
/// is visible and the mic is ON; starts the animation once sound crosses the
/// threshold.
fn wave_quiet_tick() {
    let (style, on, visible, level_type, threshold) = {
        let a = app_mut();
        (
            a.cfg.visual_style,
            a.mic == MicState::On,
            a.cfg.overlay_visible,
            a.cfg.level_type,
            a.cfg.speak_threshold_db as f32,
        )
    };
    if style != VisualStyle::Wave || !on || !visible {
        return;
    }
    let Some(channels) = wave_channels() else {
        return;
    };
    let Some(norm) = level_norm(channels, level_type, threshold) else {
        return;
    };
    if norm > 0.0 {
        start_speaking(norm, channels);
    }
}

/// One animation-timer tick: read level, smooth, redraw, and stop after the
/// release window (or immediately when the mic/visibility goes away).
fn anim_tick() {
    let (style, on, visible, level_type, threshold, release_ms, fps) = {
        let a = app_mut();
        (
            a.cfg.visual_style,
            a.mic == MicState::On,
            a.cfg.overlay_visible,
            a.cfg.level_type,
            a.cfg.speak_threshold_db as f32,
            a.cfg.speak_release_ms,
            a.cfg.anim_fps,
        )
    };
    if style != VisualStyle::Wave || !on || !visible {
        stop_anim();
        redraw_locked();
        return;
    }
    let Some(channels) = app_mut().anim_channels else {
        stop_anim();
        redraw_locked();
        return;
    };
    let target = level_norm(channels, level_type, threshold).unwrap_or(0.0);
    let now = std::time::Instant::now();
    let (dt, last) = {
        let a = app_mut();
        (a.anim_last, a.anim_ms)
    };
    let dt_ms = dt
        .map(|t| now.duration_since(t).as_millis().clamp(1, 250) as u32)
        .unwrap_or(last.max(1));
    let alive = {
        let a = app_mut();
        match a.speaking.as_mut() {
            Some(speaking) => speaking.tick(target, dt_ms, release_ms),
            None => false,
        }
    };
    if alive {
        let interval = (1000 / fps.max(1)).max(10);
        {
            let a = app_mut();
            a.anim_last = Some(now);
            if a.anim_ms != interval {
                a.anim_ms = interval;
                let hwnd = a.hwnd;
                unsafe {
                    let _ = SetTimer(Some(hwnd), window::TIMER_ANIM, interval, None);
                }
            }
        }
        redraw_speaking();
    } else {
        stop_anim();
        redraw_locked();
    }
}

/// Show or hide the overlay window. Hooks, macros and the tray icon stay
/// active while it is hidden, so the mic hotkey keeps working.
fn apply_visibility(hwnd: HWND, visible: bool) {
    if !visible {
        stop_anim();
    }
    unsafe {
        let _ = ShowWindow(hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
    }
}

fn refresh_tray() {
    if !app_mut().tray_ready {
        return;
    }
    let (hwnd, cfg_clone, mic_clone, bg) = {
        let a = app_mut();
        (a.hwnd, a.cfg.clone(), a.mic.clone(), a.cfg.color_bg)
    };
    let dot = vm::dot_color(&cfg_clone, &mic_clone);
    let new_icon = render::make_tray_icon(dot, bg);
    let tip = tray::tooltip_text(&cfg_clone, &mic_clone);
    // Swap icons (destroy old after modify to avoid flicker using null)
    let old = app_mut().tray_icon.replace(new_icon.unwrap_or_default());
    let cur = app_mut().tray_icon.unwrap_or_default();
    let cur_opt = if cur.is_invalid() { None } else { Some(cur) };
    tray::modify_tray(hwnd, cur_opt, &tip);
    if let Some(o) = old {
        if !o.is_invalid() && o != cur {
            unsafe {
                let _ = DestroyIcon(o);
            }
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
        if app_mut().mic != MicState::On {
            stop_anim();
        }
        redraw_locked();
        refresh_tray();
    }
}

/// Perform one switch press and return the raw A/B states from before the
/// write (used by hold restore). None when either button is unreadable.
fn switch_now() -> Option<(bool, bool)> {
    let (primary, secondary) = {
        let c = &app_mut().cfg;
        (c.vm_param.clone(), c.vm_param_b.clone())
    };
    app_mut().vm.switch_pair(&primary, &secondary)
}

/// Run the configured action via the Remote API, then poll immediately so
/// the overlay reflects the result. Silent on any failure.
fn do_toggle() {
    let mode = app_mut().cfg.vm_mode;
    match mode {
        VmMode::Toggle => {
            let param = app_mut().cfg.vm_param.clone();
            app_mut().vm.toggle_param(&param);
        }
        VmMode::Switch => {
            switch_now();
        }
    }
    do_poll_and_update();
}

/// Raw value that renders as display ON/OFF (accounts for `invert`,
/// which is display-only).
fn raw_for_display(display_on: bool, invert: bool) -> f32 {
    let on_raw = if invert { 0.0 } else { 1.0 };
    if display_on {
        on_raw
    } else {
        1.0 - on_raw
    }
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
    if app_mut().cfg.vm_mode == VmMode::Switch {
        // Momentary A/B: swap now, restore both raw states on release.
        let Some(saved) = switch_now() else {
            return; // either button unavailable: don't latch a hold
        };
        {
            let a = app_mut();
            a.hold_held = true;
            a.hold_saved_switch = Some(saved);
            a.hold_down_at = Some(std::time::Instant::now());
        }
        do_poll_and_update();
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
    let (mode, primary, secondary, invert, saved_on, saved_switch) = {
        let a = app_mut();
        let s = a.hold_saved_on;
        let sw = a.hold_saved_switch;
        a.hold_held = false;
        a.hold_saved_on = None;
        a.hold_saved_switch = None;
        a.hold_down_at = None;
        (
            a.cfg.vm_mode,
            a.cfg.vm_param.clone(),
            a.cfg.vm_param_b.clone(),
            a.cfg.invert,
            s,
            sw,
        )
    };
    match mode {
        VmMode::Toggle => {
            if let Some(saved_on) = saved_on {
                app_mut()
                    .vm
                    .set_raw(&primary, raw_for_display(saved_on, invert));
                do_poll_and_update();
            }
        }
        VmMode::Switch => {
            if let Some(saved) = saved_switch {
                app_mut().vm.restore_pair(&primary, &secondary, saved);
                do_poll_and_update();
            }
        }
    }
}

/// Key-up in Both mode: tap (< hold_tap_ms) toggles from the saved state,
/// longer hold restores it (momentary).
fn do_hold_up_both() {
    if !app_mut().hold_held {
        return;
    }
    let (mode, primary, secondary, invert, saved_on, saved_switch, thresh) = {
        let a = app_mut();
        let dur_ms = a
            .hold_down_at
            .map(|t| std::time::Instant::now().duration_since(t).as_millis() as u64)
            .unwrap_or(u64::MAX);
        let s = a.hold_saved_on;
        let sw = a.hold_saved_switch;
        let th = a.cfg.hold_tap_ms as u64;
        a.hold_held = false;
        a.hold_saved_on = None;
        a.hold_saved_switch = None;
        a.hold_down_at = None;
        (
            a.cfg.vm_mode,
            a.cfg.vm_param.clone(),
            a.cfg.vm_param_b.clone(),
            a.cfg.invert,
            s,
            sw,
            (dur_ms, th),
        )
    };
    let (dur_ms, thresh_ms) = thresh;
    match mode {
        VmMode::Toggle => {
            if let Some(saved_on) = saved_on {
                let target_on = if dur_ms < thresh_ms {
                    !saved_on // tap: toggle
                } else {
                    saved_on // hold: momentary, restore
                };
                app_mut()
                    .vm
                    .set_raw(&primary, raw_for_display(target_on, invert));
                do_poll_and_update();
            }
        }
        VmMode::Switch => {
            // Key-down already swapped. A tap keeps the new side; a long
            // hold restores both raw states like a momentary A/B switch.
            if let Some(saved) = saved_switch {
                if dur_ms >= thresh_ms {
                    app_mut().vm.restore_pair(&primary, &secondary, saved);
                    do_poll_and_update();
                }
            }
        }
    }
}

/// Clear hold tracking without touching Voicemeeter (used on config
/// reload where the param itself may have changed).
fn reset_hold_tracking() {
    let a = app_mut();
    a.hold_held = false;
    a.hold_saved_on = None;
    a.hold_saved_switch = None;
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

/// Index-aligned trigger list for the hook. Built from cfg.macros.
fn macro_triggers(cfg: &AppConfig) -> Vec<Trigger> {
    cfg.macros.iter().map(|m| m.trigger).collect()
}

/// Reparse macro scripts and reset per-macro runtime state. Called whenever
/// cfg.macros changes; scripts that fail to parse become no-ops (the settings
/// window validates them before saving).
fn rebuild_macro_state(a: &mut AppState) {
    a.macro_steps = a
        .cfg
        .macros
        .iter()
        .map(|m| {
            (
                parse_macro_script(&m.press).unwrap_or_default(),
                parse_macro_script(&m.release).unwrap_or_default(),
            )
        })
        .collect();
    a.macro_runtime = a
        .cfg
        .macros
        .iter()
        .map(|_| MacroRuntime::default())
        .collect();
}

/// Execute parsed Voicemeeter requests in order. Silent on any failure.
fn run_steps(steps: &[MacroStep]) {
    for step in steps {
        match step.op {
            MacroOp::Set(value) => {
                app_mut().vm.set_raw(&step.param, value);
            }
            MacroOp::Toggle => {
                let param = step.param.clone();
                app_mut().vm.toggle_param(&param);
            }
            MacroOp::Add(delta) => run_relative(&step.param, delta),
            MacroOp::Sub(delta) => run_relative(&step.param, -delta),
        }
    }
}

/// Read-modify-write for `+=` / `-=` requests.
fn run_relative(param: &str, delta: f32) {
    if let Some(current) = app_mut().vm.read_raw(param) {
        app_mut().vm.set_raw(param, current + delta);
    }
}

/// Snapshot the raw values touched by a macro so Hold/Both can restore them
/// when no release script is configured.
fn snapshot_params(steps: &[MacroStep]) -> Vec<(String, f32)> {
    let mut saved: Vec<(String, f32)> = Vec::new();
    for step in steps {
        if saved.iter().any(|(param, _)| param == &step.param) {
            continue;
        }
        if let Some(value) = app_mut().vm.read_raw(&step.param) {
            saved.push((step.param.clone(), value));
        }
    }
    saved
}

fn do_macro_down(index: usize) {
    if index >= app_mut().cfg.macros.len() || index >= app_mut().macro_steps.len() {
        return;
    }
    if app_mut().macro_runtime[index].held {
        return; // auto-repeat guard (backup for the hook-side filter)
    }
    let (mode, press_steps, has_release) = {
        let a = app_mut();
        (
            a.cfg.macros[index].mode,
            a.macro_steps[index].0.clone(),
            !a.macro_steps[index].1.is_empty(),
        )
    };
    let saved = if mode != MacroMode::Press && !has_release {
        snapshot_params(&press_steps)
    } else {
        Vec::new()
    };
    {
        let runtime = &mut app_mut().macro_runtime[index];
        runtime.held = true;
        runtime.saved = saved;
        runtime.down_at = Some(std::time::Instant::now());
    }
    run_steps(&press_steps);
    do_poll_and_update();
}

fn do_macro_up(index: usize) {
    if index >= app_mut().cfg.macros.len() || index >= app_mut().macro_runtime.len() {
        return;
    }
    let (mode, release_steps, saved, down_at, held) = {
        let a = app_mut();
        let runtime = &a.macro_runtime[index];
        (
            a.cfg.macros[index].mode,
            a.macro_steps[index].1.clone(),
            runtime.saved.clone(),
            runtime.down_at,
            runtime.held,
        )
    };
    if !held {
        return;
    }
    {
        let runtime = &mut app_mut().macro_runtime[index];
        runtime.held = false;
        runtime.saved.clear();
        runtime.down_at = None;
    }
    let fire_release = match mode {
        MacroMode::Press => false,
        MacroMode::Hold => true,
        MacroMode::Both => {
            let threshold = app_mut().cfg.hold_tap_ms as u64;
            down_at
                .map(|t| t.elapsed().as_millis() as u64 >= threshold)
                .unwrap_or(true)
        }
    };
    if !fire_release {
        return;
    }
    if release_steps.is_empty() {
        for (param, value) in &saved {
            app_mut().vm.set_raw(param, *value);
        }
    } else {
        run_steps(&release_steps);
    }
    do_poll_and_update();
}

/// Release every held macro with its restore/release script (reload, quit).
fn restore_macros_if_held() {
    let held: Vec<usize> = (0..app_mut().macro_runtime.len())
        .filter(|&i| app_mut().macro_runtime[i].held)
        .collect();
    for index in held {
        do_macro_up(index);
    }
}

/// Drop macro tracking without touching Voicemeeter (config changed).
fn reset_macro_tracking() {
    for runtime in app_mut().macro_runtime.iter_mut() {
        runtime.held = false;
        runtime.saved.clear();
        runtime.down_at = None;
    }
}

fn on_key_down(action: usize) {
    if action == hotkey::ACTION_TOGGLE {
        match app_mut().cfg.trigger_mode {
            TriggerMode::Toggle => do_toggle(),
            TriggerMode::Hold | TriggerMode::Both => do_hold_down(),
        }
    } else {
        do_macro_down(action - 1);
    }
}

fn on_key_up(action: usize) {
    if action == hotkey::ACTION_TOGGLE {
        match app_mut().cfg.trigger_mode {
            TriggerMode::Toggle => {}
            TriggerMode::Hold => do_hold_up_restore(),
            TriggerMode::Both => do_hold_up_both(),
        }
    } else {
        do_macro_up(action - 1);
    }
}

fn begin_capture(target: usize) {
    hotkey::set_capturing(true);
    {
        let a = app_mut();
        a.capture_target = target;
        a.capture_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(CAPTURE_TIMEOUT_MS));
    }
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
    stop_anim();
    reset_hold_tracking();
    reset_macro_tracking();
    rebuild_macro_state(app_mut());
    let (hwnd, x, y, size, locked, topmost, poll_ms, visible) = {
        let a = app_mut();
        (
            a.hwnd,
            a.cfg.x,
            a.cfg.y,
            a.cfg.size,
            a.cfg.locked,
            a.cfg.always_on_top,
            a.cfg.poll_ms,
            a.cfg.overlay_visible,
        )
    };
    // Reload is not a user-chosen move: keep the live position and only
    // nudge if the new size no longer fits. clamp_position() would teleport
    // to (40,40) whenever the window is partly off-screen.
    let (cx, cy) = nudge_into_view(x, y, size);
    {
        let a = app_mut();
        a.cfg.x = cx;
        a.cfg.y = cy;
    }
    apply_pos(hwnd, cx, cy, size);
    apply_lock(hwnd, locked);
    apply_topmost(hwnd, topmost);
    apply_visibility(hwnd, visible);
    // Keep the hook shadow triggers in sync (bind/macros may have come from
    // the file).
    {
        let bind = app_mut().cfg.toggle_trigger();
        let macros = macro_triggers(&app_mut().cfg);
        hotkey::set_bind(bind, macros);
    }
    unsafe {
        let _ = KillTimer(Some(hwnd), window::TIMER_POLL);
        let _ = SetTimer(Some(hwnd), window::TIMER_POLL, poll_ms, None);
    }
    // Force fresh poll so green/red reacts immediately to param/invert change.
    do_poll_and_update();
    redraw_locked();
    refresh_tray();
    // Deliberately no write on reload: onair-settings preserves comments and
    // unknown keys, and the overlay only owns position/lock/topmost/hotkey.
    // Geometry nudges are persisted by the next drag/command/exit save.
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
        let dot = vm::dot_color(&cfg_c, &mic_c);
        let icon = render::make_tray_icon(dot, bg);
        let tip = tray::tooltip_text(&cfg_c, &mic_c);
        if let Some(ic) = icon {
            // replace stored icon
            let old = app_mut().tray_icon.replace(ic);
            if let Some(o) = old {
                if !o.is_invalid() {
                    let _ = DestroyIcon(o);
                }
            }
        }
        let cur = app_mut().tray_icon.unwrap_or_default();
        tray::add_tray(hwnd, if cur.is_invalid() { None } else { Some(cur) }, &tip);
        app_mut().tray_ready = true;
        return LRESULT(0);
    }

    // Private messages from onair-settings.exe. Registered at startup so the
    // two processes agree on the numeric IDs without sharing a header.
    let (reload_msg, capture_msg) = unsafe {
        APP.as_ref()
            .map(|a| (a.reload_msg, a.capture_msg))
            .unwrap_or((0, 0))
    };
    if reload_msg != 0 && msg == reload_msg {
        if unsafe { APP.is_some() } {
            restore_hold_if_held();
            restore_macros_if_held();
            let cfg = AppConfig::load();
            app_mut().cfg = cfg;
            apply_all_after_config_change();
        }
        return LRESULT(0);
    }
    if capture_msg != 0 && msg == capture_msg {
        if unsafe { APP.is_some() } {
            restore_hold_if_held();
            restore_macros_if_held();
            begin_capture(wparam.0);
        }
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
            // May arrive during CreateWindowExW, before APP exists.
            if unsafe { APP.is_none() } {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            if let Some((x, y)) = window_rect(hwnd) {
                let save = {
                    let a = app_mut();
                    a.cfg.x = x;
                    a.cfg.y = y;
                    a.cfg.clone()
                };
                let _ = save.save_keys(POSITION_KEYS);
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
            let _ = app_mut().cfg.save_keys(POSITION_KEYS);
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            // May arrive during window creation, before APP exists.
            if unsafe { APP.is_none() } {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
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
                let _ = app_mut().cfg.save_keys(POSITION_KEYS);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == window::TIMER_POLL {
                do_poll_and_update();
                wave_quiet_tick();
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
                    let ticks_needed = (2000u32 / poll_ms.max(1)).max(1);
                    let tick = unsafe { APP.as_ref().map(|a| a.tick).unwrap_or(0) };
                    if tick >= ticks_needed {
                        app_mut().tick = 0;
                        let h1 = unsafe { APP.as_ref().map(|a| a.hwnd).unwrap_or_default() };
                        apply_topmost(h1, true);
                    }
                }
            } else if wparam.0 == window::TIMER_ANIM {
                anim_tick();
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
            // Legacy: 514 WM_LBUTTONUP / 515 WM_LBUTTONDBLCLK / 517 WM_RBUTTONUP.
            // NOTIFYICON_VERSION_4: 1024 NIN_SELECT (left),
            // 123 WM_CONTEXTMENU (right). Handle both so left-click opens
            // settings and right-click opens the menu reliably on all
            // Windows 10/11 builds.
            match mouse {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK | 1024 => {
                    tray::open_settings();
                }
                WM_RBUTTONUP | 123 => {
                    // Reentrancy guard: some systems deliver two notifications
                    // for one click; never stack two modal menus.
                    static mut IN_MENU: bool = false;
                    if unsafe { IN_MENU } {
                        return LRESULT(0);
                    }
                    unsafe {
                        IN_MENU = true;
                    }
                    let cfg_c = app_mut().cfg.clone();
                    let cmd = if cfg_c.menu_style == MenuStyle::Glass {
                        glassmenu::show(hwnd, &cfg_c)
                    } else {
                        tray::show_menu(hwnd, &cfg_c)
                    };
                    unsafe {
                        IN_MENU = false;
                    }
                    if let Some(id) = cmd {
                        handle_command(hwnd, id);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        m if m == window::WM_KEY_DOWN => {
            // Global trigger pressed (event already passed through to the
            // system by the hook proc). wparam is the action id.
            on_key_down(wparam.0);
            LRESULT(0)
        }
        m if m == window::WM_KEY_UP => {
            // Global trigger released.
            on_key_up(wparam.0);
            LRESULT(0)
        }
        m if m == window::WM_KEY_CAPTURED => {
            if hotkey::is_capturing() {
                let kind = hotkey::captured_kind(wparam.0);
                let code = lparam.0 as usize;
                let mods = hotkey::captured_mods(wparam.0);
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
                    let trigger = Trigger { key, mods };
                    let target = app_mut().capture_target;
                    if target == 0 {
                        {
                            let a = app_mut();
                            a.cfg.toggle_key = key;
                            a.cfg.toggle_mods = mods;
                        }
                        let macros = macro_triggers(&app_mut().cfg);
                        hotkey::set_bind(trigger, macros);
                        let _ = app_mut().cfg.save_keys(&[ConfigKey::ToggleKey]);
                    } else {
                        // Hand the trigger to onair-settings, which owns the
                        // macro list. Writing it here would clobber unsaved
                        // drafts in the settings window.
                        let index = target - 1;
                        app_mut().cfg.macro_capture = format!("{index}|{}", trigger.name());
                        let _ = app_mut().cfg.save_keys(&[ConfigKey::MacroCapture]);
                    }
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
                restore_macros_if_held();
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // Safety net for any destruction path that did not go through
            // ID_QUIT (for example DefWindowProc handling WM_CLOSE).
            restore_hold_if_held();
            restore_macros_if_held();
            app_mut().vm.shutdown();
            app_mut().surface = None;
            unsafe {
                let _ = KillTimer(Some(hwnd), window::TIMER_POLL);
                let _ = KillTimer(Some(hwnd), window::TIMER_ANIM);
            }
            hotkey::shutdown();
            tray::remove_tray(hwnd);
            // VmClient::drop does Logout + FreeLibrary.
            // Destroy stored icon.
            if let Some(ic) = app_mut().tray_icon.take() {
                if !ic.is_invalid() {
                    unsafe {
                        let _ = DestroyIcon(ic);
                    }
                }
            }
            unsafe {
                PostQuitMessage(0);
            }
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
        restore_macros_if_held();
        let _ = app_mut().cfg.save_keys(OVERLAY_KEYS);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return;
    }
    if id == ID_LOCK {
        let v = !app_mut().cfg.locked;
        app_mut().cfg.locked = v;
        apply_lock(hwnd, v);
        redraw_locked();
        let _ = app_mut().cfg.save_keys(&[ConfigKey::Locked]);
        return;
    }
    if id == ID_TOPMOST {
        let v = !app_mut().cfg.always_on_top;
        app_mut().cfg.always_on_top = v;
        apply_topmost(hwnd, v);
        let _ = app_mut().cfg.save_keys(&[ConfigKey::AlwaysOnTop]);
        return;
    }
    if id == ID_VISIBLE {
        let v = !app_mut().cfg.overlay_visible;
        app_mut().cfg.overlay_visible = v;
        apply_visibility(hwnd, v);
        let _ = app_mut().cfg.save_keys(&[ConfigKey::OverlayVisible]);
        return;
    }
    if id == ID_SETTINGS {
        tray::open_settings();
        return;
    }

    let style = match id {
        ID_STYLE_DOT => Some(VisualStyle::Dot),
        ID_STYLE_RING => Some(VisualStyle::Ring),
        ID_STYLE_BAR => Some(VisualStyle::Bar),
        ID_STYLE_MINI => Some(VisualStyle::Mini),
        ID_STYLE_WAVE => Some(VisualStyle::Wave),
        _ => None,
    };
    if let Some(style) = style {
        if app_mut().cfg.visual_style != style {
            app_mut().cfg.visual_style = style;
            stop_anim();
            redraw_locked();
            let _ = app_mut().cfg.save_keys(&[ConfigKey::VisualStyle]);
            // Start speaking right away when the mic is already hot.
            wave_quiet_tick();
        }
        return;
    }

    // Wave tuning: sensitivity, bar count and frame rate. Values persist and
    // take effect on the next frame (the animation reads them every tick).
    let mut saved: Option<ConfigKey> = None;
    match id {
        ID_WAVE_SENS_35 => {
            app_mut().cfg.speak_threshold_db = -35;
            saved = Some(ConfigKey::SpeakThresholdDb);
        }
        ID_WAVE_SENS_40 => {
            app_mut().cfg.speak_threshold_db = -40;
            saved = Some(ConfigKey::SpeakThresholdDb);
        }
        ID_WAVE_SENS_45 => {
            app_mut().cfg.speak_threshold_db = -45;
            saved = Some(ConfigKey::SpeakThresholdDb);
        }
        ID_WAVE_SENS_50 => {
            app_mut().cfg.speak_threshold_db = -50;
            saved = Some(ConfigKey::SpeakThresholdDb);
        }
        ID_WAVE_SENS_55 => {
            app_mut().cfg.speak_threshold_db = -55;
            saved = Some(ConfigKey::SpeakThresholdDb);
        }
        ID_WAVE_BARS_3 => {
            app_mut().cfg.wave_bars = 3;
            saved = Some(ConfigKey::WaveBars);
        }
        ID_WAVE_BARS_5 => {
            app_mut().cfg.wave_bars = 5;
            saved = Some(ConfigKey::WaveBars);
        }
        ID_WAVE_BARS_7 => {
            app_mut().cfg.wave_bars = 7;
            saved = Some(ConfigKey::WaveBars);
        }
        ID_WAVE_FPS_20 => {
            app_mut().cfg.anim_fps = 20;
            saved = Some(ConfigKey::AnimFps);
        }
        ID_WAVE_FPS_30 => {
            app_mut().cfg.anim_fps = 30;
            saved = Some(ConfigKey::AnimFps);
        }
        ID_WAVE_FPS_45 => {
            app_mut().cfg.anim_fps = 45;
            saved = Some(ConfigKey::AnimFps);
        }
        _ => {}
    }
    if let Some(key) = saved {
        // A threshold or bar-count change should be visible immediately.
        if app_mut().speaking.is_some() {
            redraw_speaking();
        } else {
            redraw_locked();
            wave_quiet_tick();
        }
        let _ = app_mut().cfg.save_keys(&[key]);
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

        let title = to_wide("OnAir");
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
        let reload_msg = window::register_message(window::RELOAD_CONFIG_MSG);
        let capture_msg = window::register_message(window::BEGIN_CAPTURE_MSG);
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
            capture_target: 0,
            macro_steps: Vec::new(),
            macro_runtime: Vec::new(),
            hold_held: false,
            hold_saved_on: None,
            hold_saved_switch: None,
            hold_down_at: None,
            reload_msg,
            capture_msg,
            pixels: Vec::new(),
            surface: None,
            speaking: None,
            anim_channels: None,
            anim_last: None,
            anim_ms: 0,
        });
        rebuild_macro_state(app_mut());

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
            let dot = vm::dot_color(&a.cfg, &a.mic);
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

        let show = if app_mut().cfg.overlay_visible {
            SW_SHOWNOACTIVATE
        } else {
            SW_HIDE
        };
        let _ = ShowWindow(hwnd, show);
        // NOTE: no SetFocus / SetActiveWindow anywhere. All SetWindowPos use NOACTIVATE.

        // Global keybind hooks (low-level keyboard + mouse).
        // Installed just before the message loop so hook events can only
        // arrive once we are pumping messages. Inactive if it fails.
        {
            let bind = app_mut().cfg.toggle_trigger();
            let macros = macro_triggers(&app_mut().cfg);
            hotkey::init(hwnd, hinstance, bind, macros);
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
            let _ = a.cfg.save_keys(OVERLAY_KEYS);
        }
    }
}
