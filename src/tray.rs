use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

use crate::config::{AppConfig, TriggerMode, VisualStyle};
use crate::vm::MicState;
use crate::window::{to_wide, WM_TRAY};

// Menu IDs
pub const ID_LOCK: u32 = 1000;
pub const ID_TOPMOST: u32 = 1001;
pub const ID_SIZE_BASE: u32 = 1100; // +0..5 -> 24,32,48,64,96,128
pub const ID_OPACITY_BASE: u32 = 1200; // +0..8 -> 100..20
pub const ID_STRIP_BASE: u32 = 1300; // +0..7 -> Strip 1..8
pub const ID_BTN_BASE: u32 = 1400; // +0..8 -> A1..A5,B1..B3,Mute
pub const ID_INVERT: u32 = 1410;
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
    match state {
        MicState::On => format!("OnAir: {} ON", cfg.vm_param),
        MicState::Off => format!("OnAir: {} OFF", cfg.vm_param),
        MicState::Unknown(reason) => {
            if reason == "Voicemeeter not running" || reason == "Voicemeeter not installed" {
                "OnAir: Voicemeeter not running".to_string()
            } else if reason == "Unknown parameter" {
                format!("OnAir: {} unknown parameter", cfg.vm_param)
            } else {
                format!("OnAir: {} ?", cfg.vm_param)
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

        // Lock / topmost
        append_text(
            hmenu,
            ID_LOCK,
            "Lock position",
            if cfg.locked { MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0) } else { MF_STRING },
        );
        append_text(
            hmenu,
            ID_TOPMOST,
            "Always on top",
            if cfg.always_on_top { MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0) } else { MF_STRING },
        );
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, None);

        // Size submenu
        let hsize = CreatePopupMenu().unwrap_or_default();
        for (i, s) in SIZES.iter().enumerate() {
            let checked = *s == cfg.size;
            append_text(
                hsize,
                ID_SIZE_BASE + i as u32,
                &format!("{} px", s),
                if checked { MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0) } else { MF_STRING },
            );
        }
        {
            let w = to_wide("Size");
            let _ = AppendMenuW(hmenu, MF_POPUP, hsize.0 as usize, PCWSTR(w.as_ptr()));
        }

        // Opacity submenu
        let hop = CreatePopupMenu().unwrap_or_default();
        for (i, o) in OPACITIES.iter().enumerate() {
            let checked = *o == cfg.opacity;
            append_text(
                hop,
                ID_OPACITY_BASE + i as u32,
                &format!("{} %", o),
                if checked { MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0) } else { MF_STRING },
            );
        }
        {
            let w = to_wide("Opacity");
            let _ = AppendMenuW(hmenu, MF_POPUP, hop.0 as usize, PCWSTR(w.as_ptr()));
        }

        // Visual style submenu (small, non-intrusive overlays)
        let hvis = CreatePopupMenu().unwrap_or_default();
        for (i, (style, label)) in VISUALS.iter().enumerate() {
            let checked = *style == cfg.visual_style;
            append_text(
                hvis,
                ID_VISUAL_BASE + i as u32,
                label,
                if checked { MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0) } else { MF_STRING },
            );
        }
        {
            let w = to_wide("Visual");
            let _ = AppendMenuW(hmenu, MF_POPUP, hvis.0 as usize, PCWSTR(w.as_ptr()));
        }

        // Background transparency submenu (bg square only,
        // foreground dot/ring/bar stays opaque).
        let hbg = CreatePopupMenu().unwrap_or_default();
        if !BG_OPACITIES.contains(&cfg.bg_opacity) {
            append_text(
                hbg,
                0xFFFC,
                &format!("Custom: {} %", cfg.bg_opacity),
                MENU_ITEM_FLAGS(MF_STRING.0 | MF_GRAYED.0),
            );
            let _ = AppendMenuW(hbg, MF_SEPARATOR, 0, None);
        }
        for (i, o) in BG_OPACITIES.iter().enumerate() {
            let checked = *o == cfg.bg_opacity;
            let label = if *o == 0 { "Transparent (no bg)".to_string() } else { format!("{} %", o) };
            append_text(
                hbg,
                ID_BG_BASE + i as u32,
                &label,
                if checked { MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0) } else { MF_STRING },
            );
        }
        {
            let w = to_wide("Background");
            let _ = AppendMenuW(hmenu, MF_POPUP, hbg.0 as usize, PCWSTR(w.as_ptr()));
        }
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, None);

        // Voicemeeter source submenu
        let hvm = CreatePopupMenu().unwrap_or_default();
        let parsed = parse_strip_button(&cfg.vm_param);
        if parsed.is_none() {
            // Disabled label "Custom: <param>"
            append_text(
                hvm,
                0xFFFF,
                &format!("Custom: {}", cfg.vm_param),
                MENU_ITEM_FLAGS(MF_STRING.0 | MF_GRAYED.0),
            );
            let _ = AppendMenuW(hvm, MF_SEPARATOR, 0, None);
        }
        // Strip submenu (radio)
        let hstrip = CreatePopupMenu().unwrap_or_default();
        let cur_strip = parsed.as_ref().map(|(s, _)| *s).unwrap_or(0);
        for n in 1..=8u32 {
            append_text(
                hstrip,
                ID_STRIP_BASE + (n - 1),
                &format!("Strip {}", n),
                if parsed.is_some() && n == cur_strip {
                    MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0)
                } else {
                    MF_STRING
                },
            );
        }
        {
            let w = to_wide("Strip");
            let _ = AppendMenuW(hvm, MF_POPUP, hstrip.0 as usize, PCWSTR(w.as_ptr()));
        }
        // Button submenu (radio)
        let hbtn = CreatePopupMenu().unwrap_or_default();
        let cur_btn = parsed.as_ref().map(|(_, b)| b.as_str()).unwrap_or("");
        for (i, b) in BUTTONS.iter().enumerate() {
            let checked = parsed.is_some() && *b == cur_btn;
            append_text(
                hbtn,
                ID_BTN_BASE + i as u32,
                b,
                if checked { MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0) } else { MF_STRING },
            );
        }
        {
            let w = to_wide("Button");
            let _ = AppendMenuW(hvm, MF_POPUP, hbtn.0 as usize, PCWSTR(w.as_ptr()));
        }
        append_text(
            hvm,
            ID_INVERT,
            "Invert",
            if cfg.invert { MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0) } else { MF_STRING },
        );
        {
            let w = to_wide("Voicemeeter source");
            let _ = AppendMenuW(hmenu, MF_POPUP, hvm.0 as usize, PCWSTR(w.as_ptr()));
        }
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, None);

        // Mic toggle keybind (flips the configured vm_param via API,
        // or push-to-talk when Trigger mode is Hold/Both).
        append_text(
            hmenu,
            0xFFFE,
            &format!("Toggle key: {}", crate::config::toggle_key_name(cfg.toggle_key)),
            MENU_ITEM_FLAGS(MF_STRING.0 | MF_GRAYED.0),
        );
        append_text(hmenu, ID_TOGGLE_SET, "Set toggle key…", MF_STRING);
        append_text(
            hmenu,
            ID_TOGGLE_CLEAR,
            "Clear toggle key",
            if cfg.toggle_key.is_none() {
                MENU_ITEM_FLAGS(MF_STRING.0 | MF_GRAYED.0)
            } else {
                MF_STRING
            },
        );
        // Trigger mode submenu: Toggle / Hold / Both.
        let htrig = CreatePopupMenu().unwrap_or_default();
        append_text(
            htrig,
            ID_TRIGGER_TOGGLE,
            "Toggle (press)",
            if cfg.trigger_mode == TriggerMode::Toggle {
                MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0)
            } else {
                MF_STRING
            },
        );
        append_text(
            htrig,
            ID_TRIGGER_HOLD,
            "Hold (push-to-talk)",
            if cfg.trigger_mode == TriggerMode::Hold {
                MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0)
            } else {
                MF_STRING
            },
        );
        append_text(
            htrig,
            ID_TRIGGER_BOTH,
            "Both (tap toggle, hold PTT)",
            if cfg.trigger_mode == TriggerMode::Both {
                MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0)
            } else {
                MF_STRING
            },
        );
        {
            let w = to_wide("Trigger mode");
            let _ = AppendMenuW(hmenu, MF_POPUP, htrig.0 as usize, PCWSTR(w.as_ptr()));
        }
        // Hold tap threshold submenu (only used by Both mode).
        let hhold = CreatePopupMenu().unwrap_or_default();
        if !HOLD_TAPS.contains(&cfg.hold_tap_ms) {
            append_text(
                hhold,
                0xFFFD,
                &format!("Custom: {} ms", cfg.hold_tap_ms),
                MENU_ITEM_FLAGS(MF_STRING.0 | MF_GRAYED.0),
            );
            let _ = AppendMenuW(hhold, MF_SEPARATOR, 0, None);
        }
        for (i, ms) in HOLD_TAPS.iter().enumerate() {
            let checked = *ms == cfg.hold_tap_ms;
            append_text(
                hhold,
                ID_HOLDMS_BASE + i as u32,
                &format!("{} ms", ms),
                if checked {
                    MENU_ITEM_FLAGS(MF_STRING.0 | MF_CHECKED.0)
                } else {
                    MF_STRING
                },
            );
        }
        {
            let w = to_wide("Hold tap threshold");
            let _ = AppendMenuW(hmenu, MF_POPUP, hhold.0 as usize, PCWSTR(w.as_ptr()));
        }
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, None);

        append_text(hmenu, ID_OPEN_CFG, "Open config file", MF_STRING);
        append_text(hmenu, ID_RELOAD_CFG, "Reload config", MF_STRING);
        append_text(hmenu, ID_QUIT, "Quit", MF_STRING);

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
