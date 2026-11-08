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
    compose_vm_param, looks_like_vm_param, parse_hex6, parse_menu_style, parse_strip_button,
    parse_toggle_key, parse_trigger_mode, parse_visual_style, parse_vm_mode, toggle_key_name,
    update_config_file, AppConfig, ConfigKey, ToggleKey,
};
use slint::{ComponentHandle, SharedString};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOWNORMAL,
};

slint::include_modules!();

const MUTEX_NAME: &str = "Local\\OnAirSettings";
const WINDOW_TITLE: &str = "OnAir Settings";
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);

/// Edits that have not been written yet. `update_config_file` re-reads the
/// file inside the write, so overlay-owned x/y/locked values, comments and
/// unknown keys are never clobbered by the settings window.
struct Pending {
    edits: HashMap<ConfigKey, String>,
    capture_started: Option<Instant>,
    capture_baseline: ToggleKey,
    capture_mtime: Option<std::time::SystemTime>,
    capture_writes: u64,
    local_writes: u64,
    synced_mtime: Option<std::time::SystemTime>,
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
        if let Ok(hwnd) = FindWindowW(None, PCWSTR(title.as_ptr())) {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
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

/// Read the capture-relevant state straight from disk. `None` means the file
/// could not be read, so a transient error must never look like a capture.
fn read_capture_state() -> Option<(ToggleKey, Option<std::time::SystemTime>)> {
    let path = AppConfig::config_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let key = AppConfig::parse(&text).toggle_key;
    let mtime = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok();
    Some((key, mtime))
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

/// Copy the loaded config into the Slint properties. Position is intentionally
/// not shown: settings never edits x/y, the overlay owns them.
fn push_config(ui: &AppWindow, cfg: &AppConfig) {
    ui.set_size_value(cfg.size as i32);
    ui.set_opacity_value(cfg.opacity as i32);
    ui.set_locked(cfg.locked);
    ui.set_stay_on_top(cfg.always_on_top);
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

    ui.set_toggle_key(toggle_key_name(cfg.toggle_key).into());
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
            let key = parse_toggle_key(value);
            ui.set_toggle_key(toggle_key_name(key).into());
            ui.set_toggle_key_set(!key.is_none());
            Ok(toggle_key_name(key))
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
    state.capture_mtime = None;
    drop(state);
    ui.set_capture_active(false);
    ui.set_capture_message(message.into());
    if error {
        show_toast(ui, "Capture did not complete.", true);
    }
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
        capture_baseline: cfg.toggle_key,
        capture_mtime: None,
        capture_writes: 0,
        local_writes: 0,
        synced_mtime: config_mtime(),
    }));
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
            if !ipc::overlay_running() {
                ui.set_overlay_running(false);
                ui.set_capture_message("OnAir is not running. Start it, then try again.".into());
                show_toast(&ui, "Start OnAir before capturing a key.", true);
                return;
            }
            // Read the baseline *before* asking the overlay to capture: a very
            // fast capture must not land between the request and our read.
            let (baseline, baseline_mtime) =
                read_capture_state().unwrap_or_else(|| (AppConfig::load().toggle_key, None));
            if !ipc::notify_begin_capture() {
                ui.set_capture_message("Could not ask OnAir to capture a key.".into());
                show_toast(&ui, "Could not start capture.", true);
                return;
            }
            let mut state = state.borrow_mut();
            state.capture_started = Some(Instant::now());
            state.capture_baseline = baseline;
            state.capture_mtime = baseline_mtime;
            state.capture_writes = state.local_writes;
            drop(state);
            ui.set_capture_active(true);
            ui.set_capture_message("".into());
            ui.set_capture_remaining(15);
            show_toast(&ui, "Waiting for a key or mouse button...", false);
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_clear_key(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut guard = state.borrow_mut();
            guard.capture_started = None;
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
                        push_config(&ui, &cfg);
                        state.borrow_mut().synced_mtime = Some(mtime);
                    }
                }
            }

            let (active, started, baseline, baseline_mtime, baseline_writes, local_writes) = {
                let state = state.borrow();
                (
                    state.capture_started.is_some(),
                    state.capture_started.unwrap_or_else(Instant::now),
                    state.capture_baseline,
                    state.capture_mtime,
                    state.capture_writes,
                    state.local_writes,
                )
            };
            if !active {
                return;
            }
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
            let Some((captured, captured_mtime)) = read_capture_state() else {
                return;
            };
            let value_changed = captured != baseline;
            // Ignore mtime changes caused by this window's own slider/segmented
            // edits while the capture is running.
            let file_changed = baseline_mtime.is_some()
                && local_writes == baseline_writes
                && captured_mtime != baseline_mtime;
            if value_changed || file_changed {
                let name = toggle_key_name(captured);
                ui.set_toggle_key(name.clone().into());
                ui.set_toggle_key_set(!captured.is_none());
                {
                    let mut state = state.borrow_mut();
                    state.capture_started = None;
                    state.capture_mtime = None;
                }
                ui.set_capture_active(false);
                ui.set_capture_message(format!("Captured: {name}").into());
                show_toast(&ui, format!("Hotkey set to {name}"), false);
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
    // so the frame styling is applied from the first UI-thread timer tick.
    let weak = ui.as_weak();
    let frame_timer = slint::Timer::default();
    frame_timer.start(
        slint::TimerMode::SingleShot,
        Duration::from_millis(80),
        move || {
            if let Some(ui) = weak.upgrade() {
                window_style::apply_frame(&ui, dark);
            }
        },
    );

    ui.run()
}
