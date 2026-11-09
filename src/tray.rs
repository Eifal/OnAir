use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{AppConfig, VmMode};
use crate::vm::MicState;
use crate::window::{to_wide, WM_TRAY};

// Menu IDs (kept stable for glass menu command dispatch)
pub const ID_LOCK: u32 = 1000;
pub const ID_TOPMOST: u32 = 1001;
pub const ID_SETTINGS: u32 = 1002;
pub const ID_QUIT: u32 = 1502;

const TRAY_UID: u32 = 1;

/// Presentation-independent description of the tray menu. Both the native
/// menu and the custom glass menu are built from this model, so checks and
/// ordering cannot drift apart.
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
    Separator,
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
}

/// Build the complete menu tree in display order.
///
/// Everything else lives in the onair-settings window; the tray menu stays
/// minimal so right-click is a quick stop, not a control panel.
pub fn build_model(cfg: &AppConfig) -> Vec<Item> {
    vec![
        Item::action(ID_SETTINGS, "Settings..."),
        Item::Separator,
        Item::check(ID_LOCK, "Lock position", cfg.locked),
        Item::check(ID_TOPMOST, "Always on top", cfg.always_on_top),
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
    fn menu_model_is_minimal_and_ordered() {
        let cfg = AppConfig {
            locked: true,
            always_on_top: false,
            ..Default::default()
        };
        let model = build_model(&cfg);
        assert_eq!(
            model,
            vec![
                Item::Action {
                    id: ID_SETTINGS,
                    label: "Settings...".into()
                },
                Item::Separator,
                Item::Check {
                    id: ID_LOCK,
                    label: "Lock position".into(),
                    checked: true
                },
                Item::Check {
                    id: ID_TOPMOST,
                    label: "Always on top".into(),
                    checked: false
                },
                Item::Separator,
                Item::Action {
                    id: ID_QUIT,
                    label: "Quit".into()
                },
            ]
        );
    }

    #[test]
    fn menu_model_has_no_submenus() {
        let model = build_model(&AppConfig::default());
        assert!(model.iter().all(|item| matches!(
            item,
            Item::Action { .. } | Item::Check { .. } | Item::Separator
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
}
