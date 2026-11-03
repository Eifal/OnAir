use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{AppConfig, MenuStyle, TriggerMode, VisualStyle, VmMode};
use crate::vm::MicState;
use crate::window::{to_wide, WM_TRAY};

// Menu IDs
pub const ID_LOCK: u32 = 1000;
pub const ID_TOPMOST: u32 = 1001;
pub const ID_SIZE_BASE: u32 = 1100; // +0..5 -> 24,32,48,64,96,128
pub const ID_OPACITY_BASE: u32 = 1200; // +0..8 -> 100..20
pub const ID_STRIP_BASE: u32 = 1300; // +0..7 -> primary Strip 1..8
pub const ID_STRIP_B_BASE: u32 = 1310; // +0..7 -> secondary Strip 1..8
pub const ID_BTN_BASE: u32 = 1400; // +0..8 -> primary A1..A5,B1..B3,Mute
pub const ID_INVERT: u32 = 1410;
pub const ID_VM_MODE_TOGGLE: u32 = 1420;
pub const ID_VM_MODE_SWITCH: u32 = 1421;
pub const ID_BTN_B_BASE: u32 = 1430; // +0..8 -> secondary A1..A5,B1..B3,Mute
pub const ID_OPEN_CFG: u32 = 1500;
pub const ID_RELOAD_CFG: u32 = 1501;
pub const ID_QUIT: u32 = 1502;
pub const ID_TOGGLE_SET: u32 = 1600;
pub const ID_TOGGLE_CLEAR: u32 = 1601;
pub const ID_TRIGGER_TOGGLE: u32 = 1610;
pub const ID_TRIGGER_HOLD: u32 = 1611;
pub const ID_TRIGGER_BOTH: u32 = 1612;
pub const ID_HOLDMS_BASE: u32 = 1620; // +0..3 -> 200,300,500,800 ms
pub const ID_VISUAL_BASE: u32 = 1700; // +0..3 -> Dot, Ring, Bar, Mini
pub const ID_BG_BASE: u32 = 1710; // +0..5 -> bg opacity presets
pub const ID_MENU_STYLE_GLASS: u32 = 1720;
pub const ID_MENU_STYLE_NATIVE: u32 = 1721;

pub const SIZES: [u32; 6] = [24, 32, 48, 64, 96, 128];
pub const OPACITIES: [u32; 9] = [100, 90, 80, 70, 60, 50, 40, 30, 20];
pub const BG_OPACITIES: [u32; 6] = [100, 90, 70, 50, 30, 0];
pub const BUTTONS: [&str; 9] = ["A1", "A2", "A3", "A4", "A5", "B1", "B2", "B3", "Mute"];
pub const HOLD_TAPS: [u32; 4] = [200, 300, 500, 800];
pub const VISUALS: [(VisualStyle, &str); 4] = [
    (VisualStyle::Dot, "Dot (filled circle)"),
    (VisualStyle::Ring, "Ring (hollow circle)"),
    (VisualStyle::Bar, "Bar (thin strip)"),
    (VisualStyle::Mini, "Mini (no background)"),
];

const TRAY_UID: u32 = 1;

/// Presentation-independent description of the tray menu. Both the native
/// menu and the custom glass menu are built from this model, so checks,
/// disabled states and ordering cannot drift apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Action {
        id: u32,
        label: String,
    },
    Check {
        id: u32,
        label: String,
        checked: bool,
    },
    Disabled {
        label: String,
    },
    Separator,
    Submenu {
        label: String,
        items: Vec<Item>,
    },
}

impl Item {
    fn action(id: u32, label: impl Into<String>) -> Self {
        Item::Action {
            id,
            label: label.into(),
        }
    }

    fn check(id: u32, label: impl Into<String>, checked: bool) -> Self {
        Item::Check {
            id,
            label: label.into(),
            checked,
        }
    }

    fn disabled(label: impl Into<String>) -> Self {
        Item::Disabled {
            label: label.into(),
        }
    }

    fn submenu(label: impl Into<String>, items: Vec<Item>) -> Self {
        Item::Submenu {
            label: label.into(),
            items,
        }
    }
}

/// Strip/Button submenus for one Voicemeeter parameter. Custom parameters
/// (e.g. Bus[1].Mute) keep a disabled label so the menu never hides them.
fn source_controls(param: &str, strip_base: u32, button_base: u32) -> Vec<Item> {
    let parsed = parse_strip_button(param);
    let mut items = Vec::new();
    if parsed.is_none() {
        items.push(Item::disabled(format!("Custom: {}", param)));
        items.push(Item::Separator);
    }
    let cur_strip = parsed.as_ref().map(|(s, _)| *s).unwrap_or(0);
    let strips: Vec<Item> = (1..=8u32)
        .map(|n| {
            Item::check(
                strip_base + (n - 1),
                format!("Strip {}", n),
                parsed.is_some() && n == cur_strip,
            )
        })
        .collect();
    items.push(Item::submenu("Strip", strips));
    let cur_btn = parsed.as_ref().map(|(_, b)| b.as_str()).unwrap_or("");
    let buttons: Vec<Item> = BUTTONS
        .iter()
        .enumerate()
        .map(|(i, b)| {
            Item::check(
                button_base + i as u32,
                *b,
                parsed.is_some() && *b == cur_btn,
            )
        })
        .collect();
    items.push(Item::submenu("Button", buttons));
    items
}

/// Build the complete menu tree in display order.
pub fn build_model(cfg: &AppConfig) -> Vec<Item> {
    let mut root = Vec::new();
    root.push(Item::check(ID_LOCK, "Lock position", cfg.locked));
    root.push(Item::check(ID_TOPMOST, "Always on top", cfg.always_on_top));
    root.push(Item::Separator);

    let sizes: Vec<Item> = SIZES
        .iter()
        .enumerate()
        .map(|(i, s)| Item::check(ID_SIZE_BASE + i as u32, format!("{} px", s), *s == cfg.size))
        .collect();
    root.push(Item::submenu("Size", sizes));

    let opacities: Vec<Item> = OPACITIES
        .iter()
        .enumerate()
        .map(|(i, o)| {
            Item::check(
                ID_OPACITY_BASE + i as u32,
                format!("{} %", o),
                *o == cfg.opacity,
            )
        })
        .collect();
    root.push(Item::submenu("Opacity", opacities));

    let visuals: Vec<Item> = VISUALS
        .iter()
        .enumerate()
        .map(|(i, (style, label))| {
            Item::check(
                ID_VISUAL_BASE + i as u32,
                *label,
                *style == cfg.visual_style,
            )
        })
        .collect();
    root.push(Item::submenu("Visual", visuals));

    let mut backgrounds = Vec::new();
    if !BG_OPACITIES.contains(&cfg.bg_opacity) {
        backgrounds.push(Item::disabled(format!("Custom: {} %", cfg.bg_opacity)));
        backgrounds.push(Item::Separator);
    }
    for (i, o) in BG_OPACITIES.iter().enumerate() {
        let label = if *o == 0 {
            "Transparent (no bg)".to_string()
        } else {
            format!("{} %", o)
        };
        backgrounds.push(Item::check(
            ID_BG_BASE + i as u32,
            label,
            *o == cfg.bg_opacity,
        ));
    }
    root.push(Item::submenu("Background", backgrounds));

    let menu_styles = vec![
        Item::check(
            ID_MENU_STYLE_GLASS,
            "Glass (acrylic)",
            cfg.menu_style == MenuStyle::Glass,
        ),
        Item::check(
            ID_MENU_STYLE_NATIVE,
            "Native (Win32)",
            cfg.menu_style == MenuStyle::Native,
        ),
    ];
    root.push(Item::submenu("Menu style", menu_styles));

    let modes = vec![
        Item::check(
            ID_VM_MODE_TOGGLE,
            "Toggle (one button on/off)",
            cfg.vm_mode == VmMode::Toggle,
        ),
        Item::check(
            ID_VM_MODE_SWITCH,
            "Switch (two buttons A/B)",
            cfg.vm_mode == VmMode::Switch,
        ),
    ];
    root.push(Item::submenu("Action mode", modes));
    root.push(Item::Separator);

    let vm = vec![
        Item::submenu(
            "Primary",
            source_controls(&cfg.vm_param, ID_STRIP_BASE, ID_BTN_BASE),
        ),
        Item::submenu(
            "Secondary",
            source_controls(&cfg.vm_param_b, ID_STRIP_B_BASE, ID_BTN_B_BASE),
        ),
        Item::check(ID_INVERT, "Invert", cfg.invert),
    ];
    root.push(Item::submenu("Voicemeeter source", vm));
    root.push(Item::Separator);

    root.push(Item::disabled(format!(
        "Toggle key: {}",
        crate::config::toggle_key_name(cfg.toggle_key)
    )));
    root.push(Item::action(ID_TOGGLE_SET, "Set toggle key…"));
    if cfg.toggle_key.is_none() {
        root.push(Item::disabled("Clear toggle key"));
    } else {
        root.push(Item::action(ID_TOGGLE_CLEAR, "Clear toggle key"));
    }

    let triggers = vec![
        Item::check(
            ID_TRIGGER_TOGGLE,
            "Toggle (press)",
            cfg.trigger_mode == TriggerMode::Toggle,
        ),
        Item::check(
            ID_TRIGGER_HOLD,
            "Hold (push-to-talk)",
            cfg.trigger_mode == TriggerMode::Hold,
        ),
        Item::check(
            ID_TRIGGER_BOTH,
            "Both (tap toggle, hold PTT)",
            cfg.trigger_mode == TriggerMode::Both,
        ),
    ];
    root.push(Item::submenu("Trigger mode", triggers));

    let mut holds = Vec::new();
    if !HOLD_TAPS.contains(&cfg.hold_tap_ms) {
        holds.push(Item::disabled(format!("Custom: {} ms", cfg.hold_tap_ms)));
        holds.push(Item::Separator);
    }
    for (i, ms) in HOLD_TAPS.iter().enumerate() {
        holds.push(Item::check(
            ID_HOLDMS_BASE + i as u32,
            format!("{} ms", ms),
            *ms == cfg.hold_tap_ms,
        ));
    }
    root.push(Item::submenu("Hold tap threshold", holds));
    root.push(Item::Separator);

    root.push(Item::action(ID_OPEN_CFG, "Open config file"));
    root.push(Item::action(ID_RELOAD_CFG, "Reload config"));
    root.push(Item::Separator);
    root.push(Item::disabled(format!(
        "OnAir {}",
        env!("CARGO_PKG_VERSION")
    )));
    root.push(Item::action(ID_QUIT, "Quit"));

    root
}

pub fn taskbar_created_msg() -> u32 {
    unsafe {
        let w = to_wide("TaskbarCreated");
        RegisterWindowMessageW(PCWSTR(w.as_ptr()))
    }
}

fn nid(hwnd: HWND, hicon: Option<HICON>, tip: &str) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_UID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        ..Default::default()
    };
    if let Some(ic) = hicon {
        nid.hIcon = ic;
    }
    let tip_w = to_wide(tip);
    let n = tip_w.len().min(nid.szTip.len());
    nid.szTip[..n].copy_from_slice(&tip_w[..n]);
    nid
}

pub fn add_tray(hwnd: HWND, hicon: Option<HICON>, tip: &str) {
    unsafe {
        let nid = nid(hwnd, hicon, tip);
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        // Request latest behavior
        let mut nid2 = nid;
        nid2.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid2);
    }
}

pub fn modify_tray(hwnd: HWND, hicon: Option<HICON>, tip: &str) {
    unsafe {
        let nid = nid(hwnd, hicon, tip);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn remove_tray(hwnd: HWND) {
    unsafe {
        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_UID,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

pub fn tooltip_text(cfg: &AppConfig, state: &MicState) -> String {
    // The dot always tracks the primary parameter; the suffix makes the
    // action mode visible without trying to fit both parameters.
    let suffix = if cfg.vm_mode == VmMode::Switch {
        " (A/B switch)"
    } else {
        ""
    };
    match state {
        MicState::On => format!("OnAir: {} ON{}", cfg.vm_param, suffix),
        MicState::Off => format!("OnAir: {} OFF{}", cfg.vm_param, suffix),
        MicState::Unknown(reason) => {
            if reason == "Voicemeeter not running" || reason == "Voicemeeter not installed" {
                "OnAir: Voicemeeter not running".to_string()
            } else if reason == "Unknown parameter" {
                format!("OnAir: {} unknown parameter{}", cfg.vm_param, suffix)
            } else {
                format!("OnAir: {} ?{}", cfg.vm_param, suffix)
            }
        }
    }
}

/// Parse "Strip[n].Btn" -> (strip_1based, button). None if custom.
pub fn parse_strip_button(param: &str) -> Option<(u32, String)> {
    // Expected: Strip[<0-based>].<Btn>
    let p = param.trim();
    if !p.starts_with("Strip[") {
        return None;
    }
    let end = p.find(']')?;
    let idx: i32 = p[6..end].parse().ok()?;
    if !(0..8).contains(&idx) {
        return None;
    }
    let rest = &p[end + 1..];
    if !rest.starts_with('.') {
        return None;
    }
    let btn = &rest[1..];
    if BUTTONS.contains(&btn) {
        Some(((idx + 1) as u32, btn.to_string()))
    } else {
        None
    }
}

fn append_text(hmenu: HMENU, id: u32, text: &str, flags: MENU_ITEM_FLAGS) {
    unsafe {
        let w = to_wide(text);
        let _ = AppendMenuW(hmenu, flags, id as usize, PCWSTR(w.as_ptr()));
    }
}

fn append_items(hmenu: HMENU, items: &[Item]) {
    for item in items {
        match item {
            Item::Action { id, label } => append_text(hmenu, *id, label, MF_STRING),
            Item::Check { id, label, checked } => {
                let flags = if *checked {
                    MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0)
                } else {
                    MF_STRING
                };
                append_text(hmenu, *id, label, flags);
            }
            Item::Disabled { label } => {
                append_text(hmenu, 0, label, MENU_ITEM_FLAGS(MF_STRING.0 | MF_GRAYED.0))
            }
            Item::Separator => unsafe {
                let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, None);
            },
            Item::Submenu { label, items } => unsafe {
                let hsub = CreatePopupMenu().unwrap_or_default();
                if hsub.is_invalid() {
                    continue;
                }
                append_items(hsub, items);
                let w = to_wide(label);
                if AppendMenuW(hmenu, MF_POPUP, hsub.0 as usize, PCWSTR(w.as_ptr())).is_err() {
                    let _ = DestroyMenu(hsub);
                }
            },
        }
    }
}

/// Show the tray menu. Returns the selected command ID, or None when the
/// menu was dismissed. The overlay itself is `WS_EX_NOACTIVATE`, so it cannot
/// be made foreground for TrackPopupMenu's outside-click dismissal. Use a
/// tiny hidden helper window as the menu owner instead, then restore the
/// previous foreground window.
pub fn show_menu(hwnd: HWND, cfg: &AppConfig) -> Option<u32> {
    unsafe {
        let hmenu = CreatePopupMenu().unwrap_or_default();
        if hmenu.is_invalid() {
            return None;
        }
        append_items(hmenu, &build_model(cfg));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);

        // A helper window with normal activation semantics. Kept off-screen
        // and 1x1 so it never paints a visible artifact.
        let class = to_wide("STATIC");
        let title = to_wide("");
        let host = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            -32000,
            -32000,
            1,
            1,
            None,
            None,
            None,
            None,
        )
        .ok();
        let owner = host.unwrap_or(hwnd);
        let previous = GetForegroundWindow();
        if host.is_some() {
            let _ = ShowWindow(owner, SW_SHOWNOACTIVATE);
            let _ = SetForegroundWindow(owner);
        }

        let cmd = TrackPopupMenu(
            hmenu,
            TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RETURNCMD,
            pt.x,
            pt.y,
            Some(0),
            owner,
            None,
        );
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(hmenu);
        if let Some(host) = host {
            let _ = DestroyWindow(host);
            if !previous.is_invalid() && previous != host {
                let _ = SetForegroundWindow(previous);
            }
        }

        if cmd.0 == 0 {
            None
        } else {
            Some(cmd.0 as u32)
        }
    }
}

pub fn open_config_file() {
    unsafe {
        let path = AppConfig::config_path();
        // Ensure file exists so "open" always works
        if !path.exists() {
            let cfg = AppConfig::load();
            let _ = cfg.save();
        }
        let p = to_wide(path.to_string_lossy().as_ref());
        let op = to_wide("open");
        let _ = ShellExecuteW(
            None,
            PCWSTR(op.as_ptr()),
            PCWSTR(p.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_submenu<'a>(items: &'a [Item], label: &str) -> &'a [Item] {
        items
            .iter()
            .find_map(|item| match item {
                Item::Submenu { label: l, items } if l == label => Some(items.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("submenu {label} not found"))
    }

    fn is_checked(items: &[Item], id: u32) -> bool {
        items.iter().any(|item| {
            matches!(
                item,
                Item::Check {
                    id: check_id,
                    checked: true,
                    ..
                } if *check_id == id
            )
        })
    }

    #[test]
    fn menu_model_exposes_menu_style() {
        let cfg = AppConfig::default();
        let model = build_model(&cfg);
        let style = model
            .iter()
            .find_map(|item| match item {
                Item::Submenu { label, items } if label == "Menu style" => Some(items),
                _ => None,
            })
            .expect("Menu style submenu");
        assert!(style.iter().any(|item| matches!(
            item,
            Item::Check {
                id: ID_MENU_STYLE_GLASS,
                checked: true,
                ..
            }
        )));
        assert!(style.iter().any(|item| matches!(
            item,
            Item::Check {
                id: ID_MENU_STYLE_NATIVE,
                checked: false,
                ..
            }
        )));
    }

    #[test]
    fn menu_model_exposes_action_modes() {
        let mut cfg = AppConfig::default();
        let model = build_model(&cfg);
        let modes = find_submenu(&model, "Action mode");
        assert!(is_checked(modes, ID_VM_MODE_TOGGLE));
        assert!(!is_checked(modes, ID_VM_MODE_SWITCH));

        cfg.vm_mode = VmMode::Switch;
        let model = build_model(&cfg);
        let modes = find_submenu(&model, "Action mode");
        assert!(!is_checked(modes, ID_VM_MODE_TOGGLE));
        assert!(is_checked(modes, ID_VM_MODE_SWITCH));
    }

    #[test]
    fn menu_model_exposes_primary_and_secondary_sources() {
        let mut cfg = AppConfig {
            vm_param: "Strip[1].B2".to_string(),
            vm_param_b: "Strip[4].Mute".to_string(),
            ..Default::default()
        };
        let model = build_model(&cfg);
        let vm = find_submenu(&model, "Voicemeeter source");

        let primary = find_submenu(vm, "Primary");
        let primary_strip = find_submenu(primary, "Strip");
        assert!(is_checked(primary_strip, ID_STRIP_BASE + 1));
        let primary_button = find_submenu(primary, "Button");
        assert!(is_checked(primary_button, ID_BTN_BASE + 6));

        let secondary = find_submenu(vm, "Secondary");
        let secondary_strip = find_submenu(secondary, "Strip");
        assert!(is_checked(secondary_strip, ID_STRIP_B_BASE + 4));
        let secondary_button = find_submenu(secondary, "Button");
        assert!(is_checked(secondary_button, ID_BTN_B_BASE + 8));

        // Custom parameters are shown, never hidden by the strip editor.
        cfg.vm_param_b = "Bus[1].Mute".to_string();
        let model = build_model(&cfg);
        let secondary = find_submenu(find_submenu(&model, "Voicemeeter source"), "Secondary");
        assert!(secondary.iter().any(|item| matches!(
            item,
            Item::Disabled { label } if label == "Custom: Bus[1].Mute"
        )));
    }

    #[test]
    fn switch_tooltip_mentions_mode() {
        let mut cfg = AppConfig::default();
        let tip = tooltip_text(&cfg, &MicState::On);
        assert_eq!(tip, "OnAir: Strip[0].B2 ON");

        cfg.vm_mode = VmMode::Switch;
        let tip = tooltip_text(&cfg, &MicState::Off);
        assert_eq!(tip, "OnAir: Strip[0].B2 OFF (A/B switch)");
    }

    #[test]
    fn strip_button_parse() {
        assert_eq!(
            parse_strip_button("Strip[0].B2"),
            Some((1, "B2".to_string()))
        );
        assert_eq!(
            parse_strip_button(" Strip[7].Mute "),
            Some((8, "Mute".to_string()))
        );
        assert_eq!(parse_strip_button("Strip[8].B2"), None);
        assert_eq!(parse_strip_button("Strip[0].X"), None);
        assert_eq!(parse_strip_button("Bus[1].Mute"), None);
        assert_eq!(parse_strip_button("Strip[0]B2"), None);
        assert_eq!(parse_strip_button(""), None);
    }
}
