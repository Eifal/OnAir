use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{AppConfig, VisualStyle, VmMode};
use crate::vm::MicState;
use crate::window::{to_wide, WM_TRAY};

// Menu IDs (kept stable for glass menu command dispatch)
pub const ID_LOCK: u32 = 1000;
pub const ID_TOPMOST: u32 = 1001;
pub const ID_SETTINGS: u32 = 1002;
pub const ID_VISIBLE: u32 = 1003;
pub const ID_MUTE_IND: u32 = 1004;
pub const ID_QUIT: u32 = 1502;

// Visual styles.
pub const ID_STYLE_DOT: u32 = 1100;
pub const ID_STYLE_RING: u32 = 1101;
pub const ID_STYLE_BAR: u32 = 1102;
pub const ID_STYLE_MINI: u32 = 1103;
pub const ID_STYLE_WAVE: u32 = 1104;
// Wave tuning.
pub const ID_WAVE_SENS_35: u32 = 1110;
pub const ID_WAVE_SENS_40: u32 = 1111;
pub const ID_WAVE_SENS_45: u32 = 1112;
pub const ID_WAVE_SENS_50: u32 = 1113;
pub const ID_WAVE_SENS_55: u32 = 1114;
pub const ID_WAVE_BARS_3: u32 = 1120;
pub const ID_WAVE_BARS_5: u32 = 1121;
pub const ID_WAVE_BARS_7: u32 = 1122;
pub const ID_WAVE_FPS_20: u32 = 1130;
pub const ID_WAVE_FPS_30: u32 = 1131;
pub const ID_WAVE_FPS_45: u32 = 1132;

const TRAY_UID: u32 = 1;

/// Presentation-independent description of the tray menu. Both the native
/// menu and the custom glass menu are built from this model, so checks and
/// ordering cannot drift apart.
#[derive(Clone)]
pub enum Item {
    Action {
        id: u32,
        label: &'static str,
    },
    Check {
        id: u32,
        label: &'static str,
        checked: bool,
    },
    Submenu {
        label: &'static str,
        items: Vec<Item>,
    },
    Separator,
}

impl Item {
    fn action(id: u32, label: &'static str) -> Self {
        Item::Action { id, label }
    }

    fn check(id: u32, label: &'static str, checked: bool) -> Self {
        Item::Check { id, label, checked }
    }

    fn submenu(label: &'static str, items: Vec<Item>) -> Self {
        Item::Submenu { label, items }
    }
}

/// Build the complete menu tree in display order.
///
/// Everything else lives in the onair-settings window; the tray menu stays
/// minimal so right-click is a quick stop, not a control panel.
#[inline(never)]
pub fn build_model(cfg: &AppConfig) -> Vec<Item> {
    let style = |s: VisualStyle| cfg.visual_style == s;
    vec![
        Item::action(ID_SETTINGS, "Settings..."),
        Item::Separator,
        Item::submenu(
            "Visual",
            vec![
                Item::check(ID_STYLE_DOT, "Dot", style(VisualStyle::Dot)),
                Item::check(ID_STYLE_RING, "Ring", style(VisualStyle::Ring)),
                Item::check(ID_STYLE_BAR, "Bar", style(VisualStyle::Bar)),
                Item::check(ID_STYLE_MINI, "Mini", style(VisualStyle::Mini)),
                Item::check(ID_STYLE_WAVE, "Wave", style(VisualStyle::Wave)),
            ],
        ),
        Item::submenu(
            "Wave",
            vec![
                Item::check(
                    ID_WAVE_SENS_35,
                    "Sensitivity -35 dB",
                    cfg.speak_threshold_db == -35,
                ),
                Item::check(
                    ID_WAVE_SENS_40,
                    "Sensitivity -40 dB",
                    cfg.speak_threshold_db == -40,
                ),
                Item::check(
                    ID_WAVE_SENS_45,
                    "Sensitivity -45 dB",
                    cfg.speak_threshold_db == -45,
                ),
                Item::check(
                    ID_WAVE_SENS_50,
                    "Sensitivity -50 dB",
                    cfg.speak_threshold_db == -50,
                ),
                Item::check(
                    ID_WAVE_SENS_55,
                    "Sensitivity -55 dB",
                    cfg.speak_threshold_db == -55,
                ),
                Item::Separator,
                Item::check(ID_WAVE_BARS_3, "3 bars", cfg.wave_bars == 3),
                Item::check(ID_WAVE_BARS_5, "5 bars", cfg.wave_bars == 5),
                Item::check(ID_WAVE_BARS_7, "7 bars", cfg.wave_bars == 7),
                Item::Separator,
                Item::check(ID_WAVE_FPS_20, "20 FPS", cfg.anim_fps == 20),
                Item::check(ID_WAVE_FPS_30, "30 FPS", cfg.anim_fps == 30),
                Item::check(ID_WAVE_FPS_45, "45 FPS", cfg.anim_fps == 45),
            ],
        ),
        Item::Separator,
        Item::check(ID_LOCK, "Lock position", cfg.locked),
        Item::check(ID_TOPMOST, "Always on top", cfg.always_on_top),
        Item::check(ID_VISIBLE, "Show overlay", cfg.overlay_visible),
        Item::check(ID_MUTE_IND, "Mute indicator", cfg.mute_indicator),
        Item::Separator,
        Item::action(ID_QUIT, "Quit"),
    ]
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

pub fn tooltip_text(cfg: &AppConfig, state: &MicState, muted: bool) -> String {
    // The dot always tracks the primary parameter; the suffix makes the
    // action mode visible without trying to fit both parameters.
    let suffix = if cfg.vm_mode == VmMode::Switch {
        " (A/B switch)"
    } else {
        ""
    };
    let mute = if muted { " (muted)" } else { "" };
    match state {
        MicState::On => format!("OnAir: {} ON{}{}", cfg.vm_param, mute, suffix),
        MicState::Off => format!("OnAir: {} OFF{}{}", cfg.vm_param, mute, suffix),
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

/// Launch onair-settings.exe from the same directory as the overlay. The
/// settings app is single-instance as well, so a second left-click just
/// foregrounds the open window.
pub fn open_settings() {
    unsafe {
        let path = std::env::current_exe().ok().and_then(|exe| {
            exe.parent()
                .map(|dir| dir.join("onair-settings.exe"))
                .filter(|p| p.is_file())
        });
        let Some(path) = path else {
            message_box(
                "OnAir Settings was not found next to onair.exe.\n\
                 Build the workspace or copy onair-settings.exe into the same folder.",
                "OnAir",
                MB_ICONERROR | MB_OK,
            );
            return;
        };
        let p = to_wide(path.to_string_lossy().as_ref());
        let op = to_wide("open");
        let result = ShellExecuteW(
            None,
            PCWSTR(op.as_ptr()),
            PCWSTR(p.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
        if (result.0 as isize) <= 32 {
            message_box(
                &format!("Couldn't start OnAir Settings.\n{}", path.display()),
                "OnAir",
                MB_ICONERROR | MB_OK,
            );
        }
    }
}

fn message_box(text: &str, caption: &str, flags: MESSAGEBOX_STYLE) {
    unsafe {
        let text = to_wide(text);
        let caption = to_wide(caption);
        let _ = MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(caption.as_ptr()), flags);
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
            Item::Submenu { label, items } => unsafe {
                let Ok(sub) = CreatePopupMenu() else {
                    continue;
                };
                append_items(sub, items);
                let w = to_wide(label);
                let _ = AppendMenuW(
                    hmenu,
                    MF_POPUP | MF_STRING,
                    sub.0 as usize,
                    PCWSTR(w.as_ptr()),
                );
            },
            Item::Separator => unsafe {
                let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, None);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_model_has_visual_and_wave_submenus() {
        let cfg = AppConfig {
            locked: true,
            always_on_top: false,
            overlay_visible: false,
            visual_style: VisualStyle::Wave,
            speak_threshold_db: -50,
            wave_bars: 7,
            anim_fps: 45,
            ..Default::default()
        };
        let model = build_model(&cfg);
        assert!(matches!(
            &model[0],
            Item::Action {
                id: ID_SETTINGS,
                ..
            }
        ));
        let Item::Submenu {
            label: visual,
            items: styles,
        } = &model[2]
        else {
            panic!("Visual submenu missing");
        };
        assert_eq!(*visual, "Visual");
        assert!(matches!(
            &styles[4],
            Item::Check {
                id: ID_STYLE_WAVE,
                checked: true,
                ..
            }
        ));
        assert!(matches!(
            &styles[0],
            Item::Check {
                id: ID_STYLE_DOT,
                checked: false,
                ..
            }
        ));

        let Item::Submenu {
            label: wave,
            items: tuning,
        } = &model[3]
        else {
            panic!("Wave submenu missing");
        };
        assert_eq!(*wave, "Wave");
        let checked = |id: u32| {
            tuning.iter().any(|item| {
                matches!(
                    item,
                    Item::Check {
                        id: item_id,
                        checked: true,
                        ..
                    } if *item_id == id
                )
            })
        };
        assert!(checked(ID_WAVE_SENS_50));
        assert!(checked(ID_WAVE_BARS_7));
        assert!(checked(ID_WAVE_FPS_45));

        assert!(matches!(
            &model[5],
            Item::Check {
                id: ID_LOCK,
                checked: true,
                ..
            }
        ));
        assert!(matches!(
            &model[6],
            Item::Check {
                id: ID_TOPMOST,
                checked: false,
                ..
            }
        ));
        assert!(matches!(
            &model[7],
            Item::Check {
                id: ID_VISIBLE,
                checked: false,
                ..
            }
        ));
        assert!(matches!(
            &model[8],
            Item::Check {
                id: ID_MUTE_IND,
                checked: true,
                ..
            }
        ));
        assert!(matches!(
            model.last(),
            Some(Item::Action { id: ID_QUIT, .. })
        ));
    }

    #[test]
    fn menu_model_defaults_check_dot_and_default_wave_tuning() {
        let model = build_model(&AppConfig::default());
        let Item::Submenu { items: styles, .. } = &model[2] else {
            panic!("Visual submenu missing");
        };
        assert!(matches!(
            &styles[0],
            Item::Check {
                id: ID_STYLE_DOT,
                checked: true,
                ..
            }
        ));
        let Item::Submenu { items: tuning, .. } = &model[3] else {
            panic!("Wave submenu missing");
        };
        let checked = |id: u32| {
            tuning.iter().any(|item| {
                matches!(
                    item,
                    Item::Check {
                        id: item_id,
                        checked: true,
                        ..
                    } if *item_id == id
                )
            })
        };
        assert!(checked(ID_WAVE_SENS_45));
        assert!(checked(ID_WAVE_BARS_5));
        assert!(checked(ID_WAVE_FPS_30));
    }

    #[test]
    fn switch_tooltip_mentions_mode() {
        let mut cfg = AppConfig::default();
        let tip = tooltip_text(&cfg, &MicState::On, false);
        assert_eq!(tip, "OnAir: Strip[0].B2 ON");

        cfg.vm_mode = VmMode::Switch;
        let tip = tooltip_text(&cfg, &MicState::Off, false);
        assert_eq!(tip, "OnAir: Strip[0].B2 OFF (A/B switch)");

        let tip = tooltip_text(&cfg, &MicState::On, true);
        assert_eq!(tip, "OnAir: Strip[0].B2 ON (muted) (A/B switch)");
    }
}
