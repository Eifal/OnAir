#![windows_subsystem = "windows"]

mod ipc;
mod picker;
mod theme;
mod window_style;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use onair_config::{
    compose_vm_param, looks_like_vm_param, parse_hex6, parse_macro_mode, parse_macro_script,
    parse_menu_style, parse_strip_button, parse_trigger, parse_trigger_mode, parse_visual_style,
    parse_vm_mode, update_config_file, AppConfig, ConfigKey, Macro, MacroMode, Trigger,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::{AttachThreadInput, CreateMutexW, GetCurrentThreadId};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, FindWindowW, GetForegroundWindow, GetWindowThreadProcessId,
    SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOWNORMAL,
};

slint::include_modules!();

const MUTEX_NAME: &str = "Local\\OnAirSettings";
const WINDOW_TITLE: &str = "OnAir Settings";
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);

/// Which trigger a capture is filling in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureTarget {
    Toggle,
    Macro(usize),
}

impl CaptureTarget {
    /// Payload understood by the overlay: 0 = hotkey, 1 + i = macro i.
    fn payload(self) -> usize {
        match self {
            CaptureTarget::Toggle => 0,
            CaptureTarget::Macro(index) => index + 1,
        }
    }
}

/// Edits that have not been written yet. `update_config_file` re-reads the
/// file inside the write, so overlay-owned x/y/locked values, comments and
/// unknown keys are never clobbered by the settings window.
struct Pending {
    edits: HashMap<ConfigKey, String>,
    capture_started: Option<Instant>,
    capture_target: Option<CaptureTarget>,
    capture_baseline: Trigger,
    capture_handoff: String,
    capture_mtime: Option<std::time::SystemTime>,
    capture_writes: u64,
    local_writes: u64,
    synced_mtime: Option<std::time::SystemTime>,
    /// Macro drafts; written only on an explicit save or delete.
    macros: Vec<Macro>,
    macro_selected: usize,
    /// True while the draft list differs from disk. Blocks the config-file
    /// watcher from replacing unsaved edits.
    macros_dirty: bool,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Create the process-lifetime single-instance mutex. A second launch
/// restores and foregrounds the existing settings window instead.
fn acquire_single_instance() -> bool {
    let name = wide(MUTEX_NAME);
    let mutex = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) };
    if mutex.is_err() {
        return false;
    }
    // Keep the handle alive for the whole process. Dropping it would release
    // the mutex and let a second instance run concurrently.
    std::mem::forget(mutex);
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        focus_existing();
        return false;
    }
    true
}

fn focus_existing() {
    let title = wide(WINDOW_TITLE);
    unsafe {
        let Ok(hwnd) = FindWindowW(None, PCWSTR(title.as_ptr())) else {
            return;
        };
        let _ = ShowWindow(hwnd, SW_RESTORE);

        // SetForegroundWindow alone is often ignored while another process
        // owns the foreground. Briefly attach to that thread (the standard
        // fallback), raise the window, then detach again.
        let foreground = GetForegroundWindow();
        let foreground_thread = GetWindowThreadProcessId(foreground, None);
        let current_thread = GetCurrentThreadId();
        let attached = foreground_thread != 0
            && foreground_thread != current_thread
            && AttachThreadInput(current_thread, foreground_thread, true).as_bool();
        let _ = SetForegroundWindow(hwnd);
        if attached {
            let _ = AttachThreadInput(current_thread, foreground_thread, false);
        }
        let _ = BringWindowToTop(hwnd);
    }
}

fn color_hex(rgb: (u8, u8, u8)) -> String {
    format!("{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2)
}

fn slint_color(rgb: (u8, u8, u8)) -> slint::Color {
    slint::Color::from_rgb_u8(rgb.0, rgb.1, rgb.2)
}

fn show_toast(ui: &AppWindow, message: impl Into<SharedString>, error: bool) {
    ui.set_toast_text(message.into());
    ui.set_toast_error(error);
}

/// Read the trigger a capture is filling in straight from disk. `None` means
/// the file could not be read, so a transient error must never look like a
/// capture. The second value is the overlay's `macro_capture` hand-off.
fn read_capture_state(
    target: CaptureTarget,
) -> Option<(Trigger, String, Option<std::time::SystemTime>)> {
    let path = AppConfig::config_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let cfg = AppConfig::parse(&text);
    let (trigger, handoff) = match target {
        CaptureTarget::Toggle => (cfg.toggle_trigger(), String::new()),
        CaptureTarget::Macro(index) => (
            cfg.macros
                .get(index)
                .map(|m| m.trigger)
                .unwrap_or(Trigger::NONE),
            cfg.macro_capture.clone(),
        ),
    };
    let mtime = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok();
    Some((trigger, handoff, mtime))
}

/// Modification time of the config file, used to notice writes made by the
/// overlay or by hand while the settings window is open.
fn config_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(AppConfig::config_path())
        .and_then(|meta| meta.modified())
        .ok()
}

fn vm_fields(param: &str, fallback_button: &str) -> (i32, SharedString, bool) {
    match parse_strip_button(param) {
        Some((strip, button)) => (strip as i32, button.into(), false),
        None => (1, fallback_button.into(), true),
    }
}

/// Human label for a macro slot, used by the selector dropdown.
fn macro_display_name(mac: &Macro, index: usize) -> String {
    let label = mac.label.trim();
    if label.is_empty() {
        format!("Macro {}", index + 1)
    } else {
        label.to_string()
    }
}

fn push_macro_list(ui: &AppWindow, state: &Rc<RefCell<Pending>>) {
    let state = state.borrow();
    let names: Vec<SharedString> = state
        .macros
        .iter()
        .enumerate()
        .map(|(i, mac)| macro_display_name(mac, i).into())
        .collect();
    ui.set_macro_names(ModelRc::new(VecModel::from(names)));
    let selected = if state.macros.is_empty() {
        -1
    } else {
        state.macro_selected.min(state.macros.len() - 1) as i32
    };
    ui.set_macro_selected(selected);
}

fn push_macro_fields(ui: &AppWindow, state: &Rc<RefCell<Pending>>) {
    let state = state.borrow();
    let Some(mac) = state.macros.get(state.macro_selected) else {
        ui.set_macro_editing(false);
        return;
    };
    ui.set_macro_editing(true);
    ui.set_macro_label(mac.label.as_str().into());
    ui.set_macro_trigger(mac.trigger.name().into());
    ui.set_macro_trigger_set(!mac.trigger.is_none());
    ui.set_macro_mode_index(match mac.mode {
        MacroMode::Press => 0,
        MacroMode::Hold => 1,
        MacroMode::Both => 2,
    });
    ui.set_macro_press(mac.press.as_str().into());
    ui.set_macro_release(mac.release.as_str().into());
}

/// Validate every draft. Returns the first problem in user-facing form.
fn validate_macros(macros: &[Macro]) -> Result<(), String> {
    let toggle = AppConfig::load().toggle_trigger();
    let mut seen: Vec<Trigger> = Vec::new();
    for (index, mac) in macros.iter().enumerate() {
        let name = macro_display_name(mac, index);
        if mac.trigger.is_none() {
            return Err(format!("{name} has no key yet. Use Capture."));
        }
        if !toggle.is_none() && mac.trigger == toggle {
            return Err(format!("{name} uses the same key as the overlay hotkey."));
        }
        if seen.contains(&mac.trigger) {
            return Err(format!("{name} uses a key that is already bound."));
        }
        seen.push(mac.trigger);
        if mac.press.trim().is_empty() {
            return Err(format!("{name} has no press script."));
        }
        parse_macro_script(&mac.press).map_err(|err| format!("{name}: {err}"))?;
        if !mac.release.trim().is_empty() {
            parse_macro_script(&mac.release).map_err(|err| format!("{name} release: {err}"))?;
        }
    }
    Ok(())
}

/// Write the draft list to config.ini and ask the overlay to reload.
fn write_macros_now(ui: &AppWindow, state: &Rc<RefCell<Pending>>, announce: bool) -> bool {
    let macros = state.borrow().macros.clone();
    if let Err(message) = validate_macros(&macros) {
        show_toast(ui, message, true);
        return false;
    }
    match AppConfig::write_macros(&macros) {
        Ok(()) => {
            {
                let mut state = state.borrow_mut();
                state.local_writes = state.local_writes.saturating_add(1);
                state.synced_mtime = config_mtime();
                state.macros_dirty = false;
            }
            let running = ipc::notify_reload();
            ui.set_overlay_running(running);
            if announce {
                show_toast(ui, "Macros saved", false);
            }
            true
        }
        Err(err) => {
            show_toast(ui, format!("Could not save macros: {err}"), true);
            false
        }
    }
}

/// Copy the macro list from disk into the draft state.
fn sync_macros(ui: &AppWindow, state: &Rc<RefCell<Pending>>, cfg: &AppConfig) {
    {
        let mut state = state.borrow_mut();
        state.macros = cfg.macros.clone();
        state.macros_dirty = false;
        if state.macros.is_empty() {
            state.macro_selected = 0;
        } else if state.macro_selected >= state.macros.len() {
            state.macro_selected = state.macros.len() - 1;
        }
    }
    push_macro_list(ui, state);
    push_macro_fields(ui, state);
}

/// Copy the loaded config into the Slint properties. Position is intentionally
/// not shown: settings never edits x/y, the overlay owns them.
fn push_config(ui: &AppWindow, cfg: &AppConfig) {
    ui.set_size_value(cfg.size as i32);
    ui.set_opacity_value(cfg.opacity as i32);
    ui.set_locked(cfg.locked);
    ui.set_stay_on_top(cfg.always_on_top);
    ui.set_overlay_visible(cfg.overlay_visible);
    ui.set_menu_style(cfg.menu_style.as_str().into());

    ui.set_visual_style(cfg.visual_style.as_str().into());
    ui.set_bg_opacity_value(cfg.bg_opacity as i32);
    ui.set_color_on(color_hex(cfg.color_on).into());
    ui.set_color_off(color_hex(cfg.color_off).into());
    ui.set_color_unknown(color_hex(cfg.color_unknown).into());
    ui.set_color_bg(color_hex(cfg.color_bg).into());
    ui.set_preview_color_on(slint_color(cfg.color_on));
    ui.set_preview_color_off(slint_color(cfg.color_off));
    ui.set_preview_color_unknown(slint_color(cfg.color_unknown));
    ui.set_preview_color_bg(slint_color(cfg.color_bg));
    ui.set_color_on_invalid(false);
    ui.set_color_off_invalid(false);
    ui.set_color_unknown_invalid(false);
    ui.set_color_bg_invalid(false);

    ui.set_vm_mode(cfg.vm_mode.as_str().into());
    ui.set_vm_param(cfg.vm_param.as_str().into());
    ui.set_vm_param_b(cfg.vm_param_b.as_str().into());
    let (strip, button, custom) = vm_fields(&cfg.vm_param, "B2");
    ui.set_vm_strip(strip);
    ui.set_vm_button(button);
    ui.set_vm_param_custom(custom);
    let (strip_b, button_b, custom_b) = vm_fields(&cfg.vm_param_b, "B1");
    ui.set_vm_strip_b(strip_b);
    ui.set_vm_button_b(button_b);
    ui.set_vm_param_b_custom(custom_b);
    ui.set_vm_param_invalid(false);
    ui.set_vm_param_b_invalid(false);
    ui.set_poll_ms(cfg.poll_ms as i32);
    ui.set_invert(cfg.invert);

    ui.set_toggle_key(cfg.toggle_trigger().name().into());
    ui.set_toggle_key_set(!cfg.toggle_key.is_none());
    ui.set_trigger_mode(cfg.trigger_mode.as_str().into());
    ui.set_hold_ms(cfg.hold_tap_ms as i32);

    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_config_path(AppConfig::config_path().display().to_string().into());
}

fn parse_u32(value: &str) -> Option<u32> {
    value.trim().parse().ok()
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Validate and clamp one edit, update the matching UI property, and return
/// the canonical value to write.
fn normalize_edit(ui: &AppWindow, key: ConfigKey, value: &str) -> Result<String, String> {
    match key {
        ConfigKey::X | ConfigKey::Y => {
            Err("Position is controlled by dragging the overlay.".into())
        }
        ConfigKey::Size => {
            let n = parse_u32(value)
                .ok_or("Size must be a whole number.")?
                .clamp(16, 512);
            ui.set_size_value(n as i32);
            Ok(n.to_string())
        }
        ConfigKey::Opacity => {
            let n = parse_u32(value)
                .ok_or("Opacity must be a whole number.")?
                .clamp(20, 100);
            ui.set_opacity_value(n as i32);
            Ok(n.to_string())
        }
        ConfigKey::Locked => {
            let on = parse_bool(value).ok_or("Lock position must be true or false.")?;
            ui.set_locked(on);
            Ok(on.to_string())
        }
        ConfigKey::AlwaysOnTop => {
            let on = parse_bool(value).ok_or("Always on top must be true or false.")?;
            ui.set_stay_on_top(on);
            Ok(on.to_string())
        }
        ConfigKey::OverlayVisible => {
            let on = parse_bool(value).ok_or("Show overlay must be true or false.")?;
            ui.set_overlay_visible(on);
            Ok(on.to_string())
        }
        ConfigKey::VmMode => {
            let mode = parse_vm_mode(value);
            ui.set_vm_mode(mode.as_str().into());
            Ok(mode.as_str().to_string())
        }
        ConfigKey::VmParam => {
            let mut param = value.trim();
            if param.is_empty() {
                param = "Strip[0].B2";
            }
            if !looks_like_vm_param(param) {
                ui.set_vm_param_invalid(true);
                return Err("Voicemeeter parameters look like Name[index].Field.".into());
            }
            let canonical = if let Some((strip, button)) = parse_strip_button(param) {
                compose_vm_param(strip, button).unwrap_or_else(|| param.to_string())
            } else {
                param.to_string()
            };
            let (strip, button, custom) = vm_fields(&canonical, "B2");
            ui.set_vm_param(canonical.clone().into());
            ui.set_vm_strip(strip);
            ui.set_vm_button(button);
            ui.set_vm_param_custom(custom);
            ui.set_vm_param_invalid(false);
            Ok(canonical)
        }
        ConfigKey::VmParamB => {
            let mut param = value.trim();
            if param.is_empty() {
                param = "Strip[0].B1";
            }
            if !looks_like_vm_param(param) {
                ui.set_vm_param_b_invalid(true);
                return Err("Voicemeeter parameters look like Name[index].Field.".into());
            }
            let canonical = if let Some((strip, button)) = parse_strip_button(param) {
                compose_vm_param(strip, button).unwrap_or_else(|| param.to_string())
            } else {
                param.to_string()
            };
            let (strip, button, custom) = vm_fields(&canonical, "B1");
            ui.set_vm_param_b(canonical.clone().into());
            ui.set_vm_strip_b(strip);
            ui.set_vm_button_b(button);
            ui.set_vm_param_b_custom(custom);
            ui.set_vm_param_b_invalid(false);
            Ok(canonical)
        }
        ConfigKey::Invert => {
            let on = parse_bool(value).ok_or("Invert must be true or false.")?;
            ui.set_invert(on);
            Ok(on.to_string())
        }
        ConfigKey::PollMs => {
            let n = parse_u32(value)
                .ok_or("Poll interval must be a whole number.")?
                .clamp(30, 1000);
            ui.set_poll_ms(n as i32);
            Ok(n.to_string())
        }
        ConfigKey::ToggleKey => {
            let trigger = parse_trigger(value);
            ui.set_toggle_key(trigger.name().into());
            ui.set_toggle_key_set(!trigger.is_none());
            Ok(trigger.name())
        }
        ConfigKey::TriggerMode => {
            let mode = parse_trigger_mode(value);
            ui.set_trigger_mode(mode.as_str().into());
            Ok(mode.as_str().to_string())
        }
        ConfigKey::HoldTapMs => {
            let n = parse_u32(value)
                .ok_or("Hold/tap threshold must be a whole number.")?
                .clamp(100, 2000);
            ui.set_hold_ms(n as i32);
            Ok(n.to_string())
        }
        ConfigKey::VisualStyle => {
            let style = parse_visual_style(value);
            ui.set_visual_style(style.as_str().into());
            Ok(style.as_str().to_string())
        }
        ConfigKey::BgOpacity => {
            let n = parse_u32(value)
                .ok_or("Background opacity must be a whole number.")?
                .clamp(0, 100);
            ui.set_bg_opacity_value(n as i32);
            Ok(n.to_string())
        }
        ConfigKey::MenuStyle => {
            let style = parse_menu_style(value);
            ui.set_menu_style(style.as_str().into());
            Ok(style.as_str().to_string())
        }
        ConfigKey::ColorOn => normalize_color(ui, value, ColorField::On),
        ConfigKey::ColorOff => normalize_color(ui, value, ColorField::Off),
        ConfigKey::ColorUnknown => normalize_color(ui, value, ColorField::Unknown),
        ConfigKey::ColorBg => normalize_color(ui, value, ColorField::Background),
        // Internal hand-off written by the overlay; never edited here.
        ConfigKey::MacroCapture => Err("Internal capture key.".into()),
    }
}

use picker::ColorField;

fn normalize_color(ui: &AppWindow, value: &str, field: ColorField) -> Result<String, String> {
    let raw = value.trim().trim_start_matches('#');
    let Some(rgb) = parse_hex6(raw) else {
        match field {
            ColorField::On => ui.set_color_on_invalid(true),
            ColorField::Off => ui.set_color_off_invalid(true),
            ColorField::Unknown => ui.set_color_unknown_invalid(true),
            ColorField::Background => ui.set_color_bg_invalid(true),
        }
        return Err("Colours use six hex digits, for example 2ECC71.".into());
    };
    match field {
        ColorField::On => {
            ui.set_color_on_invalid(false);
            ui.set_preview_color_on(slint_color(rgb));
            ui.set_color_on(color_hex(rgb).into());
        }
        ColorField::Off => {
            ui.set_color_off_invalid(false);
            ui.set_preview_color_off(slint_color(rgb));
            ui.set_color_off(color_hex(rgb).into());
        }
        ColorField::Unknown => {
            ui.set_color_unknown_invalid(false);
            ui.set_preview_color_unknown(slint_color(rgb));
            ui.set_color_unknown(color_hex(rgb).into());
        }
        ColorField::Background => {
            ui.set_color_bg_invalid(false);
            ui.set_preview_color_bg(slint_color(rgb));
            ui.set_color_bg(color_hex(rgb).into());
        }
    }
    Ok(color_hex(rgb))
}

fn write_edits(
    ui: &AppWindow,
    pending: &Rc<RefCell<Pending>>,
    edits: &[(ConfigKey, String)],
    announce: bool,
) {
    if edits.is_empty() {
        return;
    }
    let path = AppConfig::config_path();
    match update_config_file(&path, edits) {
        Ok(()) => {
            {
                let mut state = pending.borrow_mut();
                state.local_writes = state.local_writes.saturating_add(1);
                state.synced_mtime = config_mtime();
            }
            let running = ipc::notify_reload();
            ui.set_overlay_running(running);
            if announce {
                if running {
                    show_toast(ui, "Saved", false);
                } else {
                    show_toast(
                        ui,
                        "Saved - OnAir is not running; applies on next launch",
                        false,
                    );
                }
            }
        }
        Err(err) => {
            show_toast(ui, format!("Could not save config.ini: {err}"), true);
        }
    }
}

fn flush_pending(ui: &AppWindow, pending: &Rc<RefCell<Pending>>) -> bool {
    let raw_edits: Vec<(ConfigKey, String)> = {
        let mut state = pending.borrow_mut();
        state.edits.drain().collect()
    };
    ui.set_edit_pending(false);
    if raw_edits.is_empty() {
        return false;
    }

    // Text fields can contain half-typed values while the debounce timer is
    // running. Validate once on flush, but drop invalid intermediates
    // silently: the field already turns red via the invalid flag, and
    // toasting on every 150 ms pause would spam the user. An explicit
    // Enter/commit (apply-now) still reports the error.
    let mut edits = Vec::with_capacity(raw_edits.len());
    for (key, value) in raw_edits {
        if let Ok(value) = normalize_edit(ui, key, &value) {
            edits.push((key, value));
        }
    }
    write_edits(ui, pending, &edits, false);
    true
}

fn apply_edit(
    ui: &AppWindow,
    pending: &Rc<RefCell<Pending>>,
    key: &str,
    value: &str,
    immediate: bool,
) {
    let Some(key) = ConfigKey::from_name(key) else {
        show_toast(ui, "Unknown setting; nothing was saved.", true);
        return;
    };

    if immediate {
        let canonical = match normalize_edit(ui, key, value) {
            Ok(value) => value,
            Err(message) => {
                show_toast(ui, message, true);
                return;
            }
        };
        // An explicit commit supersedes a queued value for the same field.
        let mut state = pending.borrow_mut();
        state.edits.remove(&key);
        let still_pending = !state.edits.is_empty();
        drop(state);
        ui.set_edit_pending(still_pending);
        write_edits(ui, pending, &[(key, canonical)], true);
    } else {
        // Defer validation to flush_pending: text fields are edited one
        // keystroke at a time and intermediate values are often incomplete.
        let mut state = pending.borrow_mut();
        state.edits.insert(key, value.to_string());
        drop(state);
        ui.set_edit_pending(true);
    }
}

fn finish_capture(
    ui: &AppWindow,
    pending: &Rc<RefCell<Pending>>,
    message: impl Into<SharedString>,
    error: bool,
) {
    let mut state = pending.borrow_mut();
    state.capture_started = None;
    state.capture_target = None;
    state.capture_mtime = None;
    drop(state);
    ui.set_capture_active(false);
    ui.set_capture_macro_index(-1);
    ui.set_capture_message(message.into());
    if error {
        show_toast(ui, "Capture did not complete.", true);
    }
}

/// Ask the running overlay to capture a trigger for `target`. Shared by the
/// hotkey page and the macro editor.
fn start_capture(ui: &AppWindow, state: &Rc<RefCell<Pending>>, target: CaptureTarget) {
    if !ipc::overlay_running() {
        ui.set_overlay_running(false);
        ui.set_capture_message("OnAir is not running. Start it, then try again.".into());
        show_toast(ui, "Start OnAir before capturing a key.", true);
        return;
    }
    if let CaptureTarget::Macro(_) = target {
        // Drop any stale hand-off left by an interrupted capture. Otherwise
        // capturing the same key twice would not change the value and would
        // look like a timeout.
        if update_config_file(
            &AppConfig::config_path(),
            &[(ConfigKey::MacroCapture, String::new())],
        )
        .is_ok()
        {
            let mut state = state.borrow_mut();
            state.local_writes = state.local_writes.saturating_add(1);
        }
    }
    // Read the baseline *before* asking the overlay to capture: a very
    // fast capture must not land between the request and our read.
    let (baseline, handoff, baseline_mtime) = read_capture_state(target).unwrap_or_else(|| {
        let cfg = AppConfig::load();
        let trigger = match target {
            CaptureTarget::Toggle => cfg.toggle_trigger(),
            CaptureTarget::Macro(index) => cfg
                .macros
                .get(index)
                .map(|m| m.trigger)
                .unwrap_or(Trigger::NONE),
        };
        (trigger, cfg.macro_capture.clone(), None)
    });
    if !ipc::notify_begin_capture(target.payload()) {
        ui.set_capture_message("Could not ask OnAir to capture a key.".into());
        show_toast(ui, "Could not start capture.", true);
        return;
    }
    {
        let mut state = state.borrow_mut();
        state.capture_started = Some(Instant::now());
        state.capture_target = Some(target);
        state.capture_baseline = baseline;
        state.capture_handoff = handoff;
        state.capture_mtime = baseline_mtime;
        state.capture_writes = state.local_writes;
    }
    ui.set_capture_active(true);
    ui.set_capture_macro_index(match target {
        CaptureTarget::Macro(index) => index as i32,
        CaptureTarget::Toggle => -1,
    });
    ui.set_capture_message("".into());
    ui.set_capture_remaining(15);
    show_toast(ui, "Waiting for a key or mouse button...", false);
}

/// Returns true when the shell accepted the operation. `ShellExecuteW`
/// reports failure with an HINSTANCE value <= 32 (for example when the .ini
/// association is missing), so we inspect it instead of assuming success.
fn open_path(path: &Path) -> bool {
    let operation = wide("open");
    let target = wide(&path.display().to_string());
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(target.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    (result.0 as isize) > 32
}

fn main() -> Result<(), slint::PlatformError> {
    if !acquire_single_instance() {
        return Ok(());
    }

    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()?;

    let cfg = AppConfig::load();
    let ui = AppWindow::new()?;
    ui.set_disc_image(picker::disc_image());
    push_config(&ui, &cfg);
    ui.set_overlay_running(ipc::overlay_running());

    let state = Rc::new(RefCell::new(Pending {
        edits: HashMap::new(),
        capture_started: None,
        capture_target: None,
        capture_baseline: cfg.toggle_trigger(),
        capture_handoff: String::new(),
        capture_mtime: None,
        capture_writes: 0,
        local_writes: 0,
        synced_mtime: config_mtime(),
        macros: cfg.macros.clone(),
        macro_selected: 0,
        macros_dirty: false,
    }));
    sync_macros(&ui, &state, &cfg);
    let picker = Rc::new(RefCell::new(None::<picker::PickerState>));

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_apply_now(move |key, value| {
            if let Some(ui) = weak.upgrade() {
                apply_edit(&ui, &state, key.as_str(), value.as_str(), true);
            }
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_apply_later(move |key, value| {
            if let Some(ui) = weak.upgrade() {
                apply_edit(&ui, &state, key.as_str(), value.as_str(), false);
            }
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_flush_edits(move || {
            if let Some(ui) = weak.upgrade() {
                flush_pending(&ui, &state);
            }
        });
    }

    {
        let weak = ui.as_weak();
        ui.on_open_config(move || {
            let Some(ui) = weak.upgrade() else { return };
            let path = AppConfig::config_path();
            if !path.exists() {
                // Keep the file present so the shell opens the editor instead
                // of failing silently. Defaults are written by AppConfig::load
                // on startup, but a removed file should still be recoverable.
                let mut defaults = AppConfig::default();
                defaults.clamp();
                let _ = defaults.save();
            }
            if open_path(&path) {
                show_toast(&ui, "Opening config.ini…", false);
            } else {
                show_toast(
                    &ui,
                    format!("Couldn't open config.ini. Path: {}", path.display()),
                    true,
                );
            }
        });
    }

    {
        let weak = ui.as_weak();
        ui.on_open_folder(move || {
            if weak.upgrade().is_none() {
                return;
            }
            let path = AppConfig::config_path();
            let folder = path.parent().unwrap_or_else(|| Path::new("."));
            if !open_path(folder) {
                if let Some(ui) = weak.upgrade() {
                    show_toast(&ui, format!("Couldn't open {}.", folder.display()), true);
                }
            }
        });
    }

    // Open the colour picker for one of the four Appearance colour
    // fields. Pending edits are flushed first so the wheel starts
    // from the value that is on disk, not a stale one.
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let picker = picker.clone();
        ui.on_open_picker(move |key| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(field) = picker::parse_target(&key) else {
                show_toast(&ui, "Unknown colour field.", true);
                return;
            };
            flush_pending(&ui, &state);
            let hex = picker::canonical_hex(field);
            let state_value = picker::PickerState::from_hex(field, &hex).unwrap_or_else(|| {
                picker::PickerState::from_hex(field, picker::default_hex(field))
                    .expect("built-in default is a valid colour")
            });
            {
                *picker.borrow_mut() = Some(state_value);
            }
            let borrowed = picker.borrow();
            if let Some(value) = borrowed.as_ref() {
                picker::push_props(&ui, value);
            }
            ui.set_picker_open(true);
        });
    }

    {
        let weak = ui.as_weak();
        let picker = picker.clone();
        ui.on_picker_disc_pointer(move |x, y| {
            let Some(ui) = weak.upgrade() else { return };
            let mut borrowed = picker.borrow_mut();
            let Some(value) = borrowed.as_mut() else {
                return;
            };
            let (h, s) = picker::pick_disc(x, y);
            value.h = h;
            value.s = s;
            picker::push_props(&ui, value);
        });
    }

    {
        let weak = ui.as_weak();
        let picker = picker.clone();
        ui.on_picker_value_pointer(move |y| {
            let Some(ui) = weak.upgrade() else { return };
            let mut borrowed = picker.borrow_mut();
            let Some(value) = borrowed.as_mut() else {
                return;
            };
            value.v = picker::value_from_y(y);
            picker::push_props(&ui, value);
        });
    }

    {
        let weak = ui.as_weak();
        let picker = picker.clone();
        ui.on_picker_reset(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut borrowed = picker.borrow_mut();
            let Some(value) = borrowed.as_mut() else {
                return;
            };
            *value = value.reset();
            picker::push_props(&ui, value);
        });
    }

    {
        let weak = ui.as_weak();
        let picker = picker.clone();
        ui.on_picker_nudge_hue(move |degrees| {
            let Some(ui) = weak.upgrade() else { return };
            let mut borrowed = picker.borrow_mut();
            let Some(value) = borrowed.as_mut() else {
                return;
            };
            picker::nudge_hue(value, degrees);
            picker::push_props(&ui, value);
        });
    }

    {
        let weak = ui.as_weak();
        let picker = picker.clone();
        ui.on_picker_nudge_saturation(move |delta| {
            let Some(ui) = weak.upgrade() else { return };
            let mut borrowed = picker.borrow_mut();
            let Some(value) = borrowed.as_mut() else {
                return;
            };
            picker::nudge_saturation(value, delta);
            picker::push_props(&ui, value);
        });
    }

    {
        let weak = ui.as_weak();
        let picker = picker.clone();
        ui.on_picker_nudge_value(move |delta| {
            let Some(ui) = weak.upgrade() else { return };
            let mut borrowed = picker.borrow_mut();
            let Some(value) = borrowed.as_mut() else {
                return;
            };
            picker::nudge_value(value, delta);
            picker::push_props(&ui, value);
        });
    }

    {
        let weak = ui.as_weak();
        let picker = picker.clone();
        ui.on_picker_cancel(move || {
            let Some(ui) = weak.upgrade() else { return };
            picker.borrow_mut().take();
            ui.set_picker_open(false);
        });
    }

    {
        let weak = ui.as_weak();
        let picker = picker.clone();
        let state = state.clone();
        ui.on_picker_use(move || {
            let Some(ui) = weak.upgrade() else { return };
            let taken = picker.borrow_mut().take();
            let Some(value) = taken else {
                ui.set_picker_open(false);
                return;
            };
            ui.set_picker_open(false);
            apply_edit(
                &ui,
                &state,
                picker::field_key(value.field),
                &value.hex(),
                true,
            );
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_begin_capture(move || {
            let Some(ui) = weak.upgrade() else { return };
            // Flush queued edits first so their write cannot be mistaken for a
            // completed capture, and so the baseline reflects what is on disk.
            flush_pending(&ui, &state);
            start_capture(&ui, &state, CaptureTarget::Toggle);
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_clear_key(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut guard = state.borrow_mut();
            guard.capture_started = None;
            guard.capture_target = None;
            guard.capture_mtime = None;
            guard.edits.remove(&ConfigKey::ToggleKey);
            drop(guard);
            ui.set_capture_active(false);
            ui.set_capture_message("Hotkey cleared.".into());
            // write_edits does not refresh UI properties; update the
            // visible bind here so the "Current bind" readout clears too.
            ui.set_toggle_key("None".into());
            ui.set_toggle_key_set(false);
            write_edits(&ui, &state, &[(ConfigKey::ToggleKey, "None".into())], true);
        });
    }

    // ---- Macro editor -----------------------------------------------------
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_macro_select(move |index| {
            let Some(ui) = weak.upgrade() else { return };
            {
                let mut state = state.borrow_mut();
                if index < 0 || index as usize >= state.macros.len() {
                    return;
                }
                state.macro_selected = index as usize;
            }
            push_macro_fields(&ui, &state);
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_macro_add(move || {
            let Some(ui) = weak.upgrade() else { return };
            {
                let mut state = state.borrow_mut();
                let index = state.macros.len();
                state.macros.push(Macro {
                    trigger: Trigger::NONE,
                    mode: MacroMode::Press,
                    label: format!("Macro {}", index + 1),
                    press: String::new(),
                    release: String::new(),
                });
                state.macro_selected = index;
                state.macros_dirty = true;
            }
            push_macro_list(&ui, &state);
            push_macro_fields(&ui, &state);
            show_toast(&ui, "Macro added. Capture a key, then save.", false);
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_macro_delete(move || {
            let Some(ui) = weak.upgrade() else { return };
            {
                let mut state = state.borrow_mut();
                if state.macros.is_empty() {
                    return;
                }
                let index = state.macro_selected.min(state.macros.len() - 1);
                state.macros.remove(index);
                if state.macro_selected >= state.macros.len() && state.macro_selected > 0 {
                    state.macro_selected -= 1;
                }
                state.macros_dirty = true;
            }
            push_macro_list(&ui, &state);
            push_macro_fields(&ui, &state);
            // Deleting is an explicit action: persist it when the rest is
            // valid. Otherwise the draft stays until the next save.
            if validate_macros(&state.borrow().macros).is_ok() {
                write_macros_now(&ui, &state, false);
            }
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_macro_field(move |field, value| {
            let is_label = field.as_str() == "label";
            {
                let mut state = state.borrow_mut();
                let index = state.macro_selected;
                let Some(mac) = state.macros.get_mut(index) else {
                    return;
                };
                match field.as_str() {
                    "label" => mac.label = value.to_string(),
                    "press" => mac.press = value.to_string(),
                    "release" => mac.release = value.to_string(),
                    "mode" => mac.mode = parse_macro_mode(value.as_str()),
                    _ => {}
                }
                state.macros_dirty = true;
            }
            if is_label {
                if let Some(ui) = weak.upgrade() {
                    push_macro_list(&ui, &state);
                }
            }
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_macro_save(move || {
            let Some(ui) = weak.upgrade() else { return };
            if write_macros_now(&ui, &state, true) {
                push_macro_list(&ui, &state);
                push_macro_fields(&ui, &state);
            }
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_macro_capture(move |index| {
            let Some(ui) = weak.upgrade() else { return };
            if index < 0 {
                return;
            }
            let index = index as usize;
            if index >= state.borrow().macros.len() {
                return;
            }
            {
                let mut state = state.borrow_mut();
                state.macro_selected = index;
            }
            push_macro_list(&ui, &state);
            push_macro_fields(&ui, &state);
            start_capture(&ui, &state, CaptureTarget::Macro(index));
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_poll_backend(move || {
            let Some(ui) = weak.upgrade() else { return };
            let running = ipc::overlay_running();
            ui.set_overlay_running(running);

            // Adopt config changes made by the overlay (tray lock/topmost,
            // dragged position) or by hand. Skip while capturing, while a
            // debounced edit is pending, or while the colour picker owns the
            // UI, so this never fights an in-progress interaction.
            let sync = {
                let state = state.borrow();
                (
                    state.capture_started.is_some(),
                    state.synced_mtime,
                    state.edits.is_empty(),
                )
            };
            if !sync.0 && sync.2 && !ui.get_edit_pending() && !ui.get_picker_open() {
                if let Some(mtime) = config_mtime() {
                    if sync.1 != Some(mtime) {
                        let cfg = AppConfig::load();
                        let macros_dirty = state.borrow().macros_dirty;
                        push_config(&ui, &cfg);
                        // Never replace unsaved macro drafts when another
                        // window (or the tray) touches the file.
                        if !macros_dirty {
                            sync_macros(&ui, &state, &cfg);
                        }
                        state.borrow_mut().synced_mtime = Some(mtime);
                    }
                }
            }

            let (
                active,
                started,
                target,
                baseline,
                baseline_handoff,
                baseline_mtime,
                baseline_writes,
                local_writes,
            ) = {
                let state = state.borrow();
                (
                    state.capture_started.is_some(),
                    state.capture_started.unwrap_or_else(Instant::now),
                    state.capture_target,
                    state.capture_baseline,
                    state.capture_handoff.clone(),
                    state.capture_mtime,
                    state.capture_writes,
                    state.local_writes,
                )
            };
            if !active {
                return;
            }
            let Some(target) = target else { return };
            if !running {
                finish_capture(&ui, &state, "OnAir closed before a key was captured.", true);
                return;
            }
            let remaining = CAPTURE_TIMEOUT.saturating_sub(started.elapsed());
            ui.set_capture_remaining(remaining.as_secs() as i32 + 1);
            if started.elapsed() >= CAPTURE_TIMEOUT {
                finish_capture(&ui, &state, "Capture timed out. Try again.", true);
                return;
            }
            // The overlay persists the new bind on capture. Detect completion
            // by value *or* file mtime: pressing the same bind again rewrites
            // the file without changing the value.
            let Some((captured, handoff, captured_mtime)) = read_capture_state(target) else {
                return;
            };
            let value_changed = captured != baseline;
            // Ignore mtime changes caused by this window's own slider/segmented
            // edits while the capture is running.
            let file_changed = baseline_mtime.is_some()
                && local_writes == baseline_writes
                && captured_mtime != baseline_mtime;
            let done = match target {
                CaptureTarget::Toggle => value_changed || file_changed,
                // Macro captures arrive through the overlay's hand-off key.
                CaptureTarget::Macro(index) => {
                    handoff != baseline_handoff && handoff.starts_with(&format!("{index}|"))
                }
            };
            if !done {
                return;
            }
            match target {
                CaptureTarget::Toggle => {
                    let name = captured.name();
                    ui.set_toggle_key(name.clone().into());
                    ui.set_toggle_key_set(!captured.is_none());
                    {
                        let mut state = state.borrow_mut();
                        state.capture_started = None;
                        state.capture_target = None;
                        state.capture_mtime = None;
                    }
                    ui.set_capture_active(false);
                    ui.set_capture_macro_index(-1);
                    ui.set_capture_message(format!("Captured: {name}").into());
                    show_toast(&ui, format!("Hotkey set to {name}"), false);
                }
                CaptureTarget::Macro(index) => {
                    let trigger = handoff
                        .split_once('|')
                        .map(|(_, name)| parse_trigger(name))
                        .unwrap_or(Trigger::NONE);
                    if trigger.is_none() {
                        return;
                    }
                    {
                        let mut state = state.borrow_mut();
                        if let Some(mac) = state.macros.get_mut(index) {
                            mac.trigger = trigger;
                        }
                        state.capture_started = None;
                        state.capture_target = None;
                        state.capture_mtime = None;
                    }
                    ui.set_capture_active(false);
                    ui.set_capture_macro_index(-1);
                    ui.set_capture_message(format!("Captured: {}", trigger.name()).into());
                    push_macro_list(&ui, &state);
                    push_macro_fields(&ui, &state);
                    if write_macros_now(&ui, &state, false) {
                        // Consume the hand-off so the next capture starts clean.
                        let _ = update_config_file(
                            &AppConfig::config_path(),
                            &[(ConfigKey::MacroCapture, String::new())],
                        );
                        {
                            let mut state = state.borrow_mut();
                            state.local_writes = state.local_writes.saturating_add(1);
                            state.synced_mtime = config_mtime();
                        }
                        let running = ipc::notify_reload();
                        ui.set_overlay_running(running);
                        show_toast(&ui, format!("Macro key set to {}", trigger.name()), false);
                    }
                }
            }
        });
    }

    ui.show()?;
    let dark = !theme::apps_use_light_theme();
    {
        let theme = ui.global::<Theme>();
        theme.set_dark(dark);
        theme.set_font_family(theme::ui_font().into());
    }

    // The winit backend creates the native window when the event loop starts,
    // so poll for the handle instead of assuming one fixed delay. Stop as soon
    // as the DWM attributes are applied, or after roughly two seconds if the
    // backend never exposes a Win32 handle.
    let frame_timer = Rc::new(slint::Timer::default());
    {
        let weak_ui = ui.as_weak();
        let timer = Rc::downgrade(&frame_timer);
        let mut attempts = 0u32;
        frame_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                let Some(ui) = weak_ui.upgrade() else {
                    return;
                };
                attempts += 1;
                if window_style::apply_frame(&ui, dark) || attempts >= 120 {
                    if let Some(timer) = timer.upgrade() {
                        timer.stop();
                    }
                }
            },
        );
    }

    ui.run()
}
