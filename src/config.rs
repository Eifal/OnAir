use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Global toggle keybind. Keyboard uses Win32 virtual-key codes,
/// mouse covers the five buttons visible to low-level hooks
/// (extra vendor buttons are invisible to Windows itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseBtn {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

impl MouseBtn {
    pub fn id(self) -> u8 {
        match self {
            MouseBtn::Left => 0,
            MouseBtn::Right => 1,
            MouseBtn::Middle => 2,
            MouseBtn::X1 => 3,
            MouseBtn::X2 => 4,
        }
    }

    pub fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(MouseBtn::Left),
            1 => Some(MouseBtn::Right),
            2 => Some(MouseBtn::Middle),
            3 => Some(MouseBtn::X1),
            4 => Some(MouseBtn::X2),
            _ => None,
        }
    }
}

/// How the toggle-key bind activates the mic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerMode {
    /// Key/button down flips the parameter (existing behavior).
    Toggle,
    /// While held the mic is forced ON, on release it returns to the
    /// state saved on key-down (push-to-talk).
    Hold,
    /// Tap (< hold_tap_ms) toggles, longer hold is momentary.
    Both,
}

impl TriggerMode {
    pub fn as_str(self) -> &'static str {
        match self {
            TriggerMode::Toggle => "toggle",
            TriggerMode::Hold => "hold",
            TriggerMode::Both => "both",
        }
    }
}

/// Parse trigger_mode values. Accepts common aliases, falls back to Toggle.
pub fn parse_trigger_mode(raw: &str) -> TriggerMode {
    let norm: String = raw
        .chars()
        .filter(|c| *c != ' ' && *c != '_' && *c != '-' && *c != '+')
        .collect::<String>()
        .to_ascii_uppercase();
    match norm.as_str() {
        "HOLD" | "PTT" | "PUSHTOTALK" | "MOMENTARY" => TriggerMode::Hold,
        "BOTH" | "TOGGLEHOLD" | "HOLDTOGGLE" | "PRESSHOLD" | "TAPANDHOLD" => TriggerMode::Both,
        // "TOGGLE", "PRESS", "TAP" and anything unknown -> Toggle.
        _ => TriggerMode::Toggle,
    }
}

/// Overlay visual style. All styles share the same square window
/// (same drag/click area), only the drawing differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualStyle {
    /// Rounded square bg + filled circle (existing look).
    Dot,
    /// Rounded square bg + colored ring, transparent center.
    Ring,
    /// Rounded square bg + thin colored strip at the bottom.
    Bar,
    /// No bg, just a small filled dot (smallest footprint).
    Mini,
}

impl VisualStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            VisualStyle::Dot => "dot",
            VisualStyle::Ring => "ring",
            VisualStyle::Bar => "bar",
            VisualStyle::Mini => "mini",
        }
    }
}

/// Parse visual_style values. Unknown falls back to Dot. Never fails.
pub fn parse_visual_style(raw: &str) -> VisualStyle {
    let norm: String = raw
        .chars()
        .filter(|c| *c != ' ' && *c != '_' && *c != '-')
        .collect::<String>()
        .to_ascii_uppercase();
    match norm.as_str() {
        "RING" | "CIRCLE" | "OUTLINE" | "DONUT" => VisualStyle::Ring,
        "BAR" | "STRIP" | "LINE" | "PILL" => VisualStyle::Bar,
        "MINI" | "MINIMAL" | "DOTONLY" | "SMALL" | "NOBG" => VisualStyle::Mini,
        // "DOT", "CIRCLE-FILLED" and anything unknown -> Dot.
        _ => VisualStyle::Dot,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleKey {
    None,
    Key(u16),
    Mouse(MouseBtn),
}

impl ToggleKey {
    pub fn is_none(self) -> bool {
        matches!(self, ToggleKey::None)
    }
}

/// Human label for menus/tooltips: "None", "F13", "A", "Mouse X1", ...
pub fn toggle_key_name(k: ToggleKey) -> String {
    match k {
        ToggleKey::None => "None".to_string(),
        ToggleKey::Mouse(b) => match b {
            MouseBtn::Left => "Mouse Left".to_string(),
            MouseBtn::Right => "Mouse Right".to_string(),
            MouseBtn::Middle => "Mouse Middle".to_string(),
            MouseBtn::X1 => "Mouse X1".to_string(),
            MouseBtn::X2 => "Mouse X2".to_string(),
        },
        ToggleKey::Key(vk) => vk_name(vk),
    }
}

/// Map left/right modifier VKs reported by low-level hooks to the generic
/// VK used by config names. This keeps a captured "Shift" bind stable
/// across restarts (the hook reports VK_LSHIFT/VK_RSHIFT, not VK_SHIFT).
pub fn normalize_vk(vk: u16) -> u16 {
    match vk {
        0xA0 | 0xA1 => 0x10, // VK_LSHIFT / VK_RSHIFT -> VK_SHIFT
        0xA2 | 0xA3 => 0x11, // VK_LCONTROL / VK_RCONTROL -> VK_CONTROL
        0xA4 | 0xA5 => 0x12, // VK_LMENU / VK_RMENU -> VK_MENU
        _ => vk,
    }
}

/// Parse config values like "None", "F13", "A", "5", "Space",
/// "MouseLeft", "Mouse X1". Unknown/empty (and Esc, reserved for
/// capture-cancel) fall back to None. Never fails.
pub fn parse_toggle_key(raw: &str) -> ToggleKey {
    let norm: String = raw
        .chars()
        .filter(|c| *c != ' ' && *c != '_' && *c != '-')
        .collect::<String>()
        .to_ascii_uppercase();
    if norm.is_empty() || norm == "NONE" || norm == "OFF" || norm == "DISABLED" {
        return ToggleKey::None;
    }
    // Fallback form emitted by vk_name(), after underscore stripping:
    // "VK_0xE2" -> "VK0XE2".
    if let Some(hex) = norm.strip_prefix("VK0X") {
        if let Ok(vk) = u16::from_str_radix(hex, 16) {
            return ToggleKey::Key(normalize_vk(vk));
        }
        return ToggleKey::None;
    }
    if let Some(btn) = norm.strip_prefix("MOUSE") {
        let b = match btn {
            "LEFT" => Some(MouseBtn::Left),
            "RIGHT" => Some(MouseBtn::Right),
            "MIDDLE" => Some(MouseBtn::Middle),
            "X1" | "B4" => Some(MouseBtn::X1),
            "X2" | "B5" => Some(MouseBtn::X2),
            _ => None,
        };
        return b.map(ToggleKey::Mouse).unwrap_or(ToggleKey::None);
    }
    // Single letter / digit.
    if norm.len() == 1 {
        let c = norm.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return ToggleKey::Key(c as u16);
        }
        return ToggleKey::None;
    }
    // F1..F24.
    if let Some(n) = norm.strip_prefix('F') {
        if let Ok(f) = n.parse::<u32>() {
            if (1..=24).contains(&f) {
                return ToggleKey::Key(0x70 + (f - 1) as u16);
            }
        }
        return ToggleKey::None;
    }
    let vk = match norm.as_str() {
        "SPACE" => Some(0x20),
        "ENTER" | "RETURN" => Some(0x0D),
        "TAB" => Some(0x09),
        "BACKSPACE" | "BACK" => Some(0x08),
        "SHIFT" => Some(0x10),
        "CTRL" | "CONTROL" => Some(0x11),
        "ALT" => Some(0x12),
        "CAPSLOCK" => Some(0x14),
        "INSERT" | "INS" => Some(0x2D),
        "DELETE" | "DEL" => Some(0x2E),
        "HOME" => Some(0x24),
        "END" => Some(0x23),
        "PAGEUP" | "PGUP" => Some(0x21),
        "PAGEDOWN" | "PGDN" | "PGDOWN" => Some(0x22),
        "LEFT" => Some(0x25),
        "UP" => Some(0x26),
        "RIGHT" => Some(0x27),
        "DOWN" => Some(0x28),
        "NUMLOCK" => Some(0x90),
        "SCROLL" | "SCROLLLOCK" => Some(0x91),
        "PAUSE" | "BREAK" => Some(0x13),
        "PRINTSCREEN" | "PRTSC" | "PRTSCR" => Some(0x2C),
        "NUMPAD0" | "NUM0" => Some(0x60),
        "NUMPAD1" | "NUM1" => Some(0x61),
        "NUMPAD2" | "NUM2" => Some(0x62),
        "NUMPAD3" | "NUM3" => Some(0x63),
        "NUMPAD4" | "NUM4" => Some(0x64),
        "NUMPAD5" | "NUM5" => Some(0x65),
        "NUMPAD6" | "NUM6" => Some(0x66),
        "NUMPAD7" | "NUM7" => Some(0x67),
        "NUMPAD8" | "NUM8" => Some(0x68),
        "NUMPAD9" | "NUM9" => Some(0x69),
        "MULTIPLY" | "NUMPAD*" | "NUM*" => Some(0x6A),
        "ADD" | "NUMPAD+" | "NUM+" | "NUMADD" => Some(0x6B),
        "SEPARATOR" => Some(0x6C),
        "SUBTRACT" | "NUMPAD-" | "NUM-" | "NUMSUB" => Some(0x6D),
        "DECIMAL" | "NUMPAD." | "NUM." | "NUMDEC" | "NUMPADDECIMAL" => Some(0x6E),
        "DIVIDE" | "NUMPAD/" | "NUM/" | "NUMDIV" => Some(0x6F),
        // Esc is reserved for capture-cancel, never bindable.
        _ => None,
    };
    vk.map(ToggleKey::Key).unwrap_or(ToggleKey::None)
}

fn vk_name(vk: u16) -> String {
    let vk = normalize_vk(vk);
    if (0x41..=0x5A).contains(&vk) || (0x30..=0x39).contains(&vk) {
        return (vk as u8 as char).to_string();
    }
    if (0x70..=0x87).contains(&vk) {
        return format!("F{}", vk - 0x70 + 1);
    }
    if (0x60..=0x69).contains(&vk) {
        return format!("Num{}", vk - 0x60);
    }
    match vk {
        0x20 => "Space".to_string(),
        0x0D => "Enter".to_string(),
        0x09 => "Tab".to_string(),
        0x08 => "Backspace".to_string(),
        0x10 => "Shift".to_string(),
        0x11 => "Ctrl".to_string(),
        0x12 => "Alt".to_string(),
        0x14 => "CapsLock".to_string(),
        0x2D => "Insert".to_string(),
        0x2E => "Delete".to_string(),
        0x24 => "Home".to_string(),
        0x23 => "End".to_string(),
        0x21 => "PageUp".to_string(),
        0x22 => "PageDown".to_string(),
        0x25 => "Left".to_string(),
        0x26 => "Up".to_string(),
        0x27 => "Right".to_string(),
        0x28 => "Down".to_string(),
        0x90 => "NumLock".to_string(),
        0x91 => "ScrollLock".to_string(),
        0x13 => "Pause".to_string(),
        0x2C => "PrintScreen".to_string(),
        0x6A => "Num*".to_string(),
        0x6B => "Num+".to_string(),
        0x6C => "Separator".to_string(),
        0x6D => "Num-".to_string(),
        0x6E => "Num.".to_string(),
        0x6F => "Num/".to_string(),
        _ => format!("VK_{:#04X}", vk),
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub x: i32,
    pub y: i32,
    pub size: u32,
    pub opacity: u32,
    pub locked: bool,
    pub always_on_top: bool,
    pub vm_param: String,
    pub invert: bool,
    pub poll_ms: u32,
    pub toggle_key: ToggleKey,
    pub trigger_mode: TriggerMode,
    pub hold_tap_ms: u32,
    pub visual_style: VisualStyle,
    pub bg_opacity: u32,
    pub color_on: (u8, u8, u8),
    pub color_off: (u8, u8, u8),
    pub color_unknown: (u8, u8, u8),
    pub color_bg: (u8, u8, u8),
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            x: 40,
            y: 40,
            size: 48,
            opacity: 90,
            locked: false,
            always_on_top: true,
            vm_param: "Strip[0].B2".to_string(),
            invert: false,
            poll_ms: 100,
            toggle_key: ToggleKey::None,
            trigger_mode: TriggerMode::Toggle,
            hold_tap_ms: 300,
            visual_style: VisualStyle::Dot,
            bg_opacity: 90,
            color_on: (0x2E, 0xCC, 0x71),
            color_off: (0xE7, 0x4C, 0x3C),
            color_unknown: (0x80, 0x80, 0x80),
            color_bg: (0x14, 0x14, 0x14),
        }
    }
}

impl AppConfig {
    pub fn clamp(&mut self) {
        self.size = self.size.clamp(16, 512);
        self.opacity = self.opacity.clamp(20, 100);
        self.poll_ms = self.poll_ms.clamp(30, 1000);
        self.hold_tap_ms = self.hold_tap_ms.clamp(100, 2000);
        self.bg_opacity = self.bg_opacity.clamp(0, 100);
        if self.vm_param.trim().is_empty() {
            self.vm_param = "Strip[0].B2".to_string();
        }
    }

    pub fn config_path() -> PathBuf {
        static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();
        CONFIG_PATH
            .get_or_init(|| {
                // 1. Try next to exe. If a config already exists there, keep
                // using it even when the directory is not writable.
                if let Some(exe_path) = exe_path() {
                    let alongside = exe_path.join("config.ini");
                    if alongside.exists() || can_write_to_dir(&exe_path) {
                        return alongside;
                    }
                }
                // 2. Fallback %APPDATA%\onair\config.ini. Keep using an
                // existing legacy vm-mic-indicator config so an upgrade does
                // not silently lose the user's settings.
                if let Some(appdata) = appdata_dir() {
                    let legacy = appdata.join("vm-mic-indicator").join("config.ini");
                    let dir = appdata.join("onair");
                    let path = dir.join("config.ini");
                    if !path.exists() && legacy.exists() {
                        return legacy;
                    }
                    let _ = std::fs::create_dir_all(&dir);
                    return path;
                }
                // 3. Last resort: current dir
                PathBuf::from("config.ini")
            })
            .clone()
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        let mut cfg = Self::default();
        let Ok(text) = std::fs::read_to_string(&path) else {
            // First run: create defaults
            cfg.clamp();
            let _ = cfg.save();
            return cfg;
        };
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let key = k.trim();
            let val = v.trim();
            match key {
                "x" => if let Ok(n) = val.parse::<i32>() { cfg.x = n; },
                "y" => if let Ok(n) = val.parse::<i32>() { cfg.y = n; },
                "size" => if let Ok(n) = val.parse::<u32>() { cfg.size = n; },
                "opacity" => if let Ok(n) = val.parse::<u32>() { cfg.opacity = n; },
                "locked" => cfg.locked = parse_bool(val, cfg.locked),
                "always_on_top" => cfg.always_on_top = parse_bool(val, cfg.always_on_top),
                "vm_param" => if !val.is_empty() { cfg.vm_param = val.to_string(); },
                "invert" => cfg.invert = parse_bool(val, cfg.invert),
                "poll_ms" => if let Ok(n) = val.parse::<u32>() { cfg.poll_ms = n; },
                "toggle_key" => cfg.toggle_key = parse_toggle_key(val),
                "trigger_mode" => cfg.trigger_mode = parse_trigger_mode(val),
                "hold_tap_ms" | "hold_ms" | "tap_ms" => if let Ok(n) = val.parse::<u32>() { cfg.hold_tap_ms = n; },
                "visual_style" | "visual" | "style" => cfg.visual_style = parse_visual_style(val),
                "bg_opacity" | "bg_alpha" | "background_opacity" => if let Ok(n) = val.parse::<u32>() { cfg.bg_opacity = n; },
                "color_on" => if let Some(c) = parse_hex6(val) { cfg.color_on = c; },
                "color_off" => if let Some(c) = parse_hex6(val) { cfg.color_off = c; },
                "color_unknown" => if let Some(c) = parse_hex6(val) { cfg.color_unknown = c; },
                "color_bg" => if let Some(c) = parse_hex6(val) { cfg.color_bg = c; },
                // Unknown keys ignored (includes legacy macro_* from v0.3.0).
                _ => {}
            }
        }
        cfg.clamp();
        cfg
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let content = format!(
            "# OnAir config (key=value)\n\
             x={}\n\
             y={}\n\
             size={}\n\
             opacity={}\n\
             locked={}\n\
             always_on_top={}\n\
             vm_param={}\n\
             invert={}\n\
             poll_ms={}\n\
             toggle_key={}\n\
             trigger_mode={}\n\
             hold_tap_ms={}\n\
             visual_style={}\n\
             bg_opacity={}\n\
             color_on={:02X}{:02X}{:02X}\n\
             color_off={:02X}{:02X}{:02X}\n\
             color_unknown={:02X}{:02X}{:02X}\n\
             color_bg={:02X}{:02X}{:02X}\n",
            self.x, self.y, self.size, self.opacity,
            self.locked, self.always_on_top,
            self.vm_param, self.invert, self.poll_ms,
            toggle_key_name(self.toggle_key),
            self.trigger_mode.as_str(), self.hold_tap_ms,
            self.visual_style.as_str(),
            self.bg_opacity,
            self.color_on.0, self.color_on.1, self.color_on.2,
            self.color_off.0, self.color_off.1, self.color_off.2,
            self.color_unknown.0, self.color_unknown.1, self.color_unknown.2,
            self.color_bg.0, self.color_bg.1, self.color_bg.2,
        );
        write_atomic(&path, content.as_bytes())
    }

    /// Dot color for current mic state.
    pub fn dot_color(&self, state: &crate::vm::MicState) -> (u8, u8, u8) {
        use crate::vm::MicState::*;
        match state {
            On => self.color_on,
            Off => self.color_off,
            Unknown(_) => self.color_unknown,
        }
    }
}

/// Write to a sibling temp file, flush it, then atomically replace the
/// destination. A crash mid-write leaves the previous config intact.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("ini.tmp");
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn parse_bool(v: &str, fallback: bool) -> bool {
    match v.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => true,
        "false" | "0" | "no" | "off" => false,
        _ => fallback,
    }
}

pub fn parse_hex6(v: &str) -> Option<(u8, u8, u8)> {
    let s = v.trim().trim_start_matches('#');
    // Slice by byte below; reject non-ASCII first so a 6-byte UTF-8
    // string cannot panic on a char boundary.
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

fn exe_path() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

fn can_write_to_dir(dir: &Path) -> bool {
    // Try creating (and removing) a temp file. If that fails, dir is not writable.
    // Also treat "config.ini already exists and is writable" via same probe.
    let probe = dir.join(".vm-mic-write-test.tmp");
    match std::fs::write(&probe, b"test") {
        Ok(()) => { let _ = std::fs::remove_file(&probe); true }
        Err(_) => false,
    }
}

fn appdata_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_key_roundtrip() {
        let cases = [
            ToggleKey::None,
            ToggleKey::Key(0x41), // A
            ToggleKey::Key(0x35), // 5
            ToggleKey::Key(0x7C), // F13
            ToggleKey::Key(0x20), // Space
            ToggleKey::Key(0x26), // Up
            ToggleKey::Key(0x10), // Shift (generic)
            ToggleKey::Key(0x11), // Ctrl (generic)
            ToggleKey::Key(0x12), // Alt (generic)
            ToggleKey::Key(0xE2), // unknown VK -> VK_0xE2 fallback
            ToggleKey::Key(0x61), // Num1
            ToggleKey::Key(0x65), // Num5
            ToggleKey::Key(0x6B), // Num+
            ToggleKey::Mouse(MouseBtn::Left),
            ToggleKey::Mouse(MouseBtn::Right),
            ToggleKey::Mouse(MouseBtn::Middle),
            ToggleKey::Mouse(MouseBtn::X1),
            ToggleKey::Mouse(MouseBtn::X2),
        ];
        for k in cases {
            assert_eq!(parse_toggle_key(&toggle_key_name(k)), k, "roundtrip {k:?}");
        }
    }

    #[test]
    fn modifier_vk_normalization_and_fallback() {
        // Low-level hooks report left/right modifiers; config names use
        // the generic VK.
        assert_eq!(normalize_vk(0xA0), 0x10); // LShift
        assert_eq!(normalize_vk(0xA1), 0x10); // RShift
        assert_eq!(normalize_vk(0xA2), 0x11); // LCtrl
        assert_eq!(normalize_vk(0xA3), 0x11); // RCtrl
        assert_eq!(normalize_vk(0xA4), 0x12); // LAlt
        assert_eq!(normalize_vk(0xA5), 0x12); // RAlt
        assert_eq!(normalize_vk(0x41), 0x41);

        assert_eq!(toggle_key_name(ToggleKey::Key(0xA2)), "Ctrl");
        assert_eq!(parse_toggle_key("VK_0xA0"), ToggleKey::Key(0x10));
        assert_eq!(parse_toggle_key("VK0XFF"), ToggleKey::Key(0xFF));
        assert_eq!(
            parse_toggle_key(&toggle_key_name(ToggleKey::Key(0xFF))),
            ToggleKey::Key(0xFF)
        );
    }

    #[test]
    fn numpad_parse() {
        assert_eq!(parse_toggle_key("Num1"), ToggleKey::Key(0x61));
        assert_eq!(parse_toggle_key("NUMPAD5"), ToggleKey::Key(0x65));
        assert_eq!(parse_toggle_key("num+"), ToggleKey::Key(0x6B));
        assert_eq!(parse_toggle_key("Num/"), ToggleKey::Key(0x6F));
        assert_eq!(toggle_key_name(ToggleKey::Key(0x61)), "Num1");
    }

    #[test]
    fn toggle_key_parse_variants() {
        assert_eq!(parse_toggle_key("None"), ToggleKey::None);
        assert_eq!(parse_toggle_key(""), ToggleKey::None);
        assert_eq!(parse_toggle_key("off"), ToggleKey::None);
        assert_eq!(parse_toggle_key("f13"), ToggleKey::Key(0x7C));
        assert_eq!(parse_toggle_key("F24"), ToggleKey::Key(0x87));
        assert_eq!(parse_toggle_key("a"), ToggleKey::Key(0x41));
        assert_eq!(parse_toggle_key("5"), ToggleKey::Key(0x35));
        assert_eq!(parse_toggle_key("space"), ToggleKey::Key(0x20));
        assert_eq!(parse_toggle_key("MouseX1"), ToggleKey::Mouse(MouseBtn::X1));
        assert_eq!(parse_toggle_key("mouse x2"), ToggleKey::Mouse(MouseBtn::X2));
        assert_eq!(parse_toggle_key("MOUSE_LEFT"), ToggleKey::Mouse(MouseBtn::Left));
        assert_eq!(parse_toggle_key("MouseB4"), ToggleKey::Mouse(MouseBtn::X1));
        assert_eq!(parse_toggle_key("MouseB5"), ToggleKey::Mouse(MouseBtn::X2));
        // Esc is reserved for capture-cancel, never bindable.
        assert_eq!(parse_toggle_key("Esc"), ToggleKey::None);
        assert_eq!(parse_toggle_key("Escape"), ToggleKey::None);
        // Garbage never fails, falls back to None.
        assert_eq!(parse_toggle_key("F99"), ToggleKey::None);
        assert_eq!(parse_toggle_key("MouseX9"), ToggleKey::None);
        assert_eq!(parse_toggle_key("???"), ToggleKey::None);
    }

    #[test]
    fn mouse_btn_ids() {
        for id in 0..5u8 {
            let b = MouseBtn::from_id(id).unwrap();
            assert_eq!(b.id(), id);
        }
        assert!(MouseBtn::from_id(5).is_none());
    }

    #[test]
    fn trigger_mode_parse() {
        assert_eq!(parse_trigger_mode("toggle"), TriggerMode::Toggle);
        assert_eq!(parse_trigger_mode("TOGGLE"), TriggerMode::Toggle);
        assert_eq!(parse_trigger_mode("press"), TriggerMode::Toggle);
        assert_eq!(parse_trigger_mode("hold"), TriggerMode::Hold);
        assert_eq!(parse_trigger_mode("HOLD"), TriggerMode::Hold);
        assert_eq!(parse_trigger_mode("ptt"), TriggerMode::Hold);
        assert_eq!(parse_trigger_mode("both"), TriggerMode::Both);
        assert_eq!(parse_trigger_mode("toggle+hold"), TriggerMode::Both);
        assert_eq!(parse_trigger_mode(""), TriggerMode::Toggle);
        assert_eq!(parse_trigger_mode("garbage"), TriggerMode::Toggle);
        assert_eq!(TriggerMode::Toggle.as_str(), "toggle");
        assert_eq!(TriggerMode::Hold.as_str(), "hold");
        assert_eq!(TriggerMode::Both.as_str(), "both");
    }

    #[test]
    fn hold_tap_ms_clamp() {
        let mut cfg = AppConfig::default();
        assert_eq!(cfg.hold_tap_ms, 300);
        cfg.hold_tap_ms = 0;
        cfg.clamp();
        assert_eq!(cfg.hold_tap_ms, 100);
        cfg.hold_tap_ms = 9999;
        cfg.clamp();
        assert_eq!(cfg.hold_tap_ms, 2000);
    }

    #[test]
    fn clamp_covers_all_ranges() {
        let mut cfg = AppConfig {
            size: 1,
            opacity: 1,
            poll_ms: 1,
            hold_tap_ms: 1,
            bg_opacity: 101,
            vm_param: "   ".to_string(),
            ..Default::default()
        };
        cfg.clamp();
        assert_eq!(cfg.size, 16);
        assert_eq!(cfg.opacity, 20);
        assert_eq!(cfg.poll_ms, 30);
        assert_eq!(cfg.hold_tap_ms, 100);
        assert_eq!(cfg.bg_opacity, 100);
        assert_eq!(cfg.vm_param, "Strip[0].B2");

        let mut cfg = AppConfig {
            size: 9999,
            opacity: 9999,
            poll_ms: 9999,
            hold_tap_ms: 9999,
            bg_opacity: 9999,
            ..Default::default()
        };
        cfg.clamp();
        assert_eq!(cfg.size, 512);
        assert_eq!(cfg.opacity, 100);
        assert_eq!(cfg.poll_ms, 1000);
        assert_eq!(cfg.hold_tap_ms, 2000);
        assert_eq!(cfg.bg_opacity, 100);
    }

    #[test]
    fn visual_style_parse() {
        assert_eq!(parse_visual_style("dot"), VisualStyle::Dot);
        assert_eq!(parse_visual_style("DOT"), VisualStyle::Dot);
        assert_eq!(parse_visual_style("ring"), VisualStyle::Ring);
        assert_eq!(parse_visual_style("circle"), VisualStyle::Ring);
        assert_eq!(parse_visual_style("bar"), VisualStyle::Bar);
        assert_eq!(parse_visual_style("strip"), VisualStyle::Bar);
        assert_eq!(parse_visual_style("pill"), VisualStyle::Bar);
        assert_eq!(parse_visual_style("mini"), VisualStyle::Mini);
        assert_eq!(parse_visual_style("minimal"), VisualStyle::Mini);
        assert_eq!(parse_visual_style(""), VisualStyle::Dot);
        assert_eq!(parse_visual_style("garbage"), VisualStyle::Dot);
        assert_eq!(VisualStyle::Dot.as_str(), "dot");
        assert_eq!(VisualStyle::Ring.as_str(), "ring");
        assert_eq!(VisualStyle::Bar.as_str(), "bar");
        assert_eq!(VisualStyle::Mini.as_str(), "mini");
    }

    #[test]
    fn hex_color_parsing_is_total() {
        assert_eq!(parse_hex6("2ECC71"), Some((0x2E, 0xCC, 0x71)));
        assert_eq!(parse_hex6("#2ecc71"), Some((0x2E, 0xCC, 0x71)));
        assert_eq!(parse_hex6("  #2ECC71  "), Some((0x2E, 0xCC, 0x71)));
        assert_eq!(parse_hex6("12345"), None);
        assert_eq!(parse_hex6("GGGGGG"), None);
        assert_eq!(parse_hex6(""), None);
        // Six bytes but only two UTF-8 characters: must not panic.
        assert_eq!(parse_hex6("€€"), None);
    }

    #[test]
    fn atomic_save_replaces_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "onair-atomic-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.ini");
        std::fs::write(&path, b"old").unwrap();

        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(!dir.join("config.ini.tmp").exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bg_opacity_clamp() {
        let mut cfg = AppConfig::default();
        assert_eq!(cfg.bg_opacity, 90);
        cfg.bg_opacity = 500;
        cfg.clamp();
        assert_eq!(cfg.bg_opacity, 100);
    }
}
