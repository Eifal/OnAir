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

/// What the toggle keybind does to Voicemeeter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmMode {
    /// Flip one parameter ON/OFF.
    Toggle,
    /// Swap two parameters so exactly one is ON.
    Switch,
}

impl VmMode {
    pub fn as_str(self) -> &'static str {
        match self {
            VmMode::Toggle => "toggle",
            VmMode::Switch => "switch",
        }
    }
}

/// Parse vm_mode values. Unknown values fall back to Toggle.
pub fn parse_vm_mode(raw: &str) -> VmMode {
    let norm: String = raw
        .chars()
        .filter(|c| !matches!(c, ' ' | '_' | '-' | '+' | '/'))
        .collect::<String>()
        .to_ascii_uppercase();
    match norm.as_str() {
        "SWITCH" | "SWAP" | "AB" | "ALTERNATE" | "ALTERNATING" | "TWOBUTTONS" => VmMode::Switch,
        // "TOGGLE", "ONOFF", "PRESS" and anything unknown -> Toggle.
        _ => VmMode::Toggle,
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
    /// Rounded square bg + animated level bars (Voicemeeter input level).
    Wave,
}

impl VisualStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            VisualStyle::Dot => "dot",
            VisualStyle::Ring => "ring",
            VisualStyle::Bar => "bar",
            VisualStyle::Mini => "mini",
            VisualStyle::Wave => "wave",
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
        "WAVE" | "VOICE" | "SPEAKING" => VisualStyle::Wave,
        // "DOT", "CIRCLE-FILLED" and anything unknown -> Dot.
        _ => VisualStyle::Dot,
    }
}

/// Tray menu presentation. Glass is the custom acrylic menu; Native keeps
/// the exact Win32 menu as a fallback and for accessibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuStyle {
    Glass,
    Native,
}

impl MenuStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            MenuStyle::Glass => "glass",
            MenuStyle::Native => "native",
        }
    }
}

/// Parse menu_style values. Unknown values fall back to Glass.
pub fn parse_menu_style(raw: &str) -> MenuStyle {
    match raw.trim().to_ascii_lowercase().as_str() {
        "native" | "classic" | "win32" | "system" => MenuStyle::Native,
        // "glass", "acrylic", "blur", "modern", "default" and anything unknown.
        _ => MenuStyle::Glass,
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

/// Modifier keys that participate in a global trigger. Left/right variants
/// are collapsed into one flag, matching `normalize_vk`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Modifiers {
    pub fn is_none(self) -> bool {
        !(self.ctrl || self.alt || self.shift || self.win)
    }
}

/// One global trigger: a base key/button plus optional modifiers. Existing
/// `toggle_key` values parse as a trigger with no modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trigger {
    pub key: ToggleKey,
    pub mods: Modifiers,
}

impl Trigger {
    pub const NONE: Trigger = Trigger {
        key: ToggleKey::None,
        mods: Modifiers {
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
        },
    };

    pub fn is_none(self) -> bool {
        self.key.is_none()
    }

    /// Canonical label: `Ctrl+Alt+Shift+Win+Key`, or `None`.
    pub fn name(self) -> String {
        if self.key.is_none() {
            return "None".to_string();
        }
        let mut parts: Vec<String> = Vec::with_capacity(5);
        if self.mods.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.mods.alt {
            parts.push("Alt".to_string());
        }
        if self.mods.shift {
            parts.push("Shift".to_string());
        }
        if self.mods.win {
            parts.push("Win".to_string());
        }
        parts.push(toggle_key_name(self.key));
        parts.join("+")
    }
}

fn is_modifier_key(key: ToggleKey) -> bool {
    match key {
        ToggleKey::Key(vk) => matches!(
            normalize_vk(vk),
            0x10 // Shift
                | 0x11 // Ctrl
                | 0x12 // Alt
                | 0x5B // LWin
                | 0x5C // RWin
        ),
        _ => false,
    }
}

/// Parse a trigger such as `F13`, `Ctrl+Alt+F13`, `Shift+Mouse X1`.
/// Modifiers must come before the base key. A pure modifier is not bindable,
/// and Esc stays reserved for capture-cancel. Never fails.
pub fn parse_trigger(raw: &str) -> Trigger {
    let mut mods = Modifiers::default();
    let mut rest = raw.trim();
    while let Some((head, tail)) = rest.split_once('+') {
        match head.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" => mods.alt = true,
            "shift" => mods.shift = true,
            "win" | "windows" | "meta" | "super" => mods.win = true,
            _ => break,
        }
        rest = tail.trim();
    }
    let key = parse_toggle_key(rest);
    if key.is_none() || is_modifier_key(key) {
        Trigger::NONE
    } else {
        Trigger { key, mods }
    }
}

/// How a macro reacts to its trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroMode {
    /// Run the press script once on key-down.
    Press,
    /// Run press on key-down and release/restore on key-up.
    Hold,
    /// Tap keeps the press result; a long hold restores it.
    Both,
}

impl MacroMode {
    pub fn as_str(self) -> &'static str {
        match self {
            MacroMode::Press => "press",
            MacroMode::Hold => "hold",
            MacroMode::Both => "both",
        }
    }
}

pub fn parse_macro_mode(raw: &str) -> MacroMode {
    let norm: String = raw
        .chars()
        .filter(|c| *c != ' ' && *c != '_' && *c != '-')
        .collect::<String>()
        .to_ascii_uppercase();
    match norm.as_str() {
        "HOLD" | "PTT" | "PUSHTOTALK" | "MOMENTARY" => MacroMode::Hold,
        "BOTH" | "TAPANDHOLD" | "TOGGLEHOLD" => MacroMode::Both,
        // "PRESS", "TAP", "TRIGGER" and anything unknown.
        _ => MacroMode::Press,
    }
}

/// One user-defined macro button. Scripts are Voicemeeter requests separated
/// by `;` or newlines; `release` may be empty, in which case Hold/Both restore
/// the parameter values saved on key-down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Macro {
    pub trigger: Trigger,
    pub mode: MacroMode,
    pub label: String,
    pub press: String,
    pub release: String,
}

/// One-line label: `|` is the row separator, so it becomes a space.
fn macro_label_field(value: &str) -> String {
    value.replace(['|', '\r', '\n'], " ").trim().to_string()
}

/// One-line script: newlines are legal separators in the editor, but a
/// `macro=` row is a single line, so they become `;`.
fn macro_script_field(value: &str) -> String {
    value
        .replace('\r', "")
        .replace('\n', ";")
        .replace('|', " ")
        .trim()
        .to_string()
}

impl Macro {
    /// Serialize as a `macro=` config line.
    pub fn encode(&self) -> String {
        format!(
            "macro={}|{}|{}|{}|{}",
            self.trigger.name(),
            self.mode.as_str(),
            macro_label_field(&self.label),
            macro_script_field(&self.press),
            macro_script_field(&self.release)
        )
    }

    /// Parse the value part of a `macro=` line. Returns None for an unusable
    /// entry (no trigger or no press script).
    pub fn parse(value: &str) -> Option<Self> {
        // Accept both the value (`F13|...`) and a full line (`macro=F13|...`)
        // so callers can round-trip `encode` output directly.
        let value = value.trim();
        let value = match value.split_once('=') {
            Some((name, rest))
                if matches!(
                    name.trim().to_ascii_lowercase().as_str(),
                    "macro" | "macros"
                ) =>
            {
                rest
            }
            _ => value,
        };
        let parts: Vec<&str> = value.splitn(5, '|').collect();
        if parts.len() < 4 {
            return None;
        }
        let trigger = parse_trigger(parts[0]);
        if trigger.is_none() {
            return None;
        }
        let press = parts[3].trim().to_string();
        if press.is_empty() {
            return None;
        }
        let label = parts[2].trim();
        Some(Self {
            trigger,
            mode: parse_macro_mode(parts[1]),
            label: if label.is_empty() {
                "Macro".to_string()
            } else {
                label.to_string()
            },
            press,
            release: parts
                .get(4)
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
        })
    }
}

/// A single request inside a macro script.
#[derive(Debug, Clone, PartialEq)]
pub enum MacroOp {
    Set(f32),
    Toggle,
    Add(f32),
    Sub(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MacroStep {
    pub param: String,
    pub op: MacroOp,
}

/// Voicemeeter numeric parameters: `Strip[0].B2`, `Bus[1].Gain`,
/// `Command.Restart`, ...
fn valid_macro_param(param: &str) -> bool {
    let Some((head, tail)) = param.split_once('.') else {
        return false;
    };
    let (name, index) = match head.split_once('[') {
        Some((name, rest)) => match rest.strip_suffix(']') {
            Some(index) => (name, Some(index)),
            None => return false,
        },
        None => (head, None),
    };
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    if let Some(index) = index {
        if index.is_empty() || !index.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    !tail.is_empty() && tail.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parse a macro script into ordered requests. Statements are separated by
/// `;` or newlines. Supported forms:
///
/// ```text
/// Strip[0].B2=1
/// Strip[0].B2=!
/// Strip[0].Gain+=1.5
/// Strip[0].Gain-=0.5
/// Command.Restart=1
/// ```
///
/// Returns the first error so the settings window can show it.
pub fn parse_macro_script(script: &str) -> Result<Vec<MacroStep>, String> {
    let mut steps = Vec::new();
    for raw in script.split([';', '\n']) {
        let statement = raw.trim();
        if statement.is_empty() || statement.starts_with('#') {
            continue;
        }
        let (param, op, value) = if let Some((p, v)) = statement.split_once("+=") {
            (p, "+", v)
        } else if let Some((p, v)) = statement.split_once("-=") {
            (p, "-", v)
        } else if let Some((p, v)) = statement.split_once('=') {
            (p, "=", v)
        } else {
            return Err(format!("Missing '=' in \"{statement}\""));
        };
        let param = param.trim();
        if !valid_macro_param(param) {
            return Err(format!("Invalid parameter \"{param}\""));
        }
        let value = value.trim();
        let op = if op == "=" && value == "!" {
            MacroOp::Toggle
        } else {
            let n: f32 = value
                .parse()
                .map_err(|_| format!("Invalid value \"{value}\" for {param}"))?;
            if !n.is_finite() {
                return Err(format!("Invalid value \"{value}\" for {param}"));
            }
            match op {
                "+" => MacroOp::Add(n),
                "-" => MacroOp::Sub(n),
                _ => MacroOp::Set(n),
            }
        };
        steps.push(MacroStep {
            param: param.to_string(),
            op,
        });
    }
    if steps.is_empty() {
        return Err("Macro has no requests".to_string());
    }
    Ok(steps)
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
    pub overlay_visible: bool,
    pub macros: Vec<Macro>,
    pub vm_mode: VmMode,
    pub vm_param: String,
    pub vm_param_b: String,
    pub invert: bool,
    pub poll_ms: u32,
    pub toggle_key: ToggleKey,
    /// Modifiers held together with `toggle_key` for the overlay hotkey.
    pub toggle_mods: Modifiers,
    /// Transient hand-off from the overlay: `"{macro index}|{trigger}"` written
    /// when a macro capture completes. Settings applies it and clears it.
    pub macro_capture: String,
    pub trigger_mode: TriggerMode,
    pub hold_tap_ms: u32,
    pub visual_style: VisualStyle,
    pub bg_opacity: u32,
    /// Wave style: level at or below this is silence (clamped -70..-20).
    pub speak_threshold_db: i32,
    /// Wave style: hangover before the animation stops (100..1500 ms).
    pub speak_release_ms: u32,
    /// Wave style animation rate (10..60 fps).
    pub anim_fps: u32,
    /// Number of animated bars (3..9).
    pub wave_bars: u32,
    /// Voicemeeter level type: 0 pre-fader, 1 post-fader, 2 post-mute, 3 output.
    pub level_type: u32,
    /// Manual input channel (left; right = +1). `None` = follow `vm_param`.
    pub level_channel: Option<u32>,
    pub menu_style: MenuStyle,
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
            overlay_visible: true,
            macros: Vec::new(),
            vm_mode: VmMode::Toggle,
            vm_param: "Strip[0].B2".to_string(),
            vm_param_b: "Strip[0].B1".to_string(),
            invert: false,
            poll_ms: 100,
            toggle_key: ToggleKey::None,
            toggle_mods: Modifiers::default(),
            macro_capture: String::new(),
            trigger_mode: TriggerMode::Toggle,
            hold_tap_ms: 300,
            visual_style: VisualStyle::Dot,
            bg_opacity: 90,
            speak_threshold_db: -45,
            speak_release_ms: 300,
            anim_fps: 30,
            wave_bars: 5,
            level_type: 0,
            level_channel: None,
            menu_style: MenuStyle::Glass,
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
        self.speak_threshold_db = self.speak_threshold_db.clamp(-70, -20);
        self.speak_release_ms = self.speak_release_ms.clamp(100, 1500);
        self.anim_fps = self.anim_fps.clamp(10, 60);
        self.wave_bars = self.wave_bars.clamp(3, 9);
        self.level_type = self.level_type.min(3);
        if let Some(channel) = self.level_channel {
            self.level_channel = Some(channel.min(4095));
        }
        if self.vm_param.trim().is_empty() {
            self.vm_param = "Strip[0].B2".to_string();
        }
        if self.vm_param_b.trim().is_empty() {
            self.vm_param_b = "Strip[0].B1".to_string();
        }
        if self.toggle_key.is_none() {
            self.toggle_mods = Modifiers::default();
        }
        // A trigger can only drive one action: one macro per key and no macro
        // on the same key as the overlay hotkey. Keep the first entry when the
        // file contains duplicates and drop unusable rows.
        let toggle = self.toggle_trigger();
        let mut seen: Vec<Trigger> = Vec::new();
        self.macros.retain(|m| {
            if m.trigger.is_none() || m.trigger == toggle || seen.contains(&m.trigger) {
                false
            } else {
                seen.push(m.trigger);
                true
            }
        });
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
        Self::load_from(&Self::config_path())
    }

    /// Load from an explicit path. Only a genuinely missing file counts as a
    /// first run; any other read error (sharing violation, permissions, invalid
    /// data) returns defaults *without writing*, so a transient failure can
    /// never delete the user's config.
    fn load_from(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // First run: create defaults
                let mut cfg = Self::default();
                cfg.clamp();
                let _ = cfg.write_full(path);
                cfg
            }
            Err(_) => {
                let mut cfg = Self::default();
                cfg.clamp();
                cfg
            }
        }
    }

    /// Parse config text. Never fails; invalid values keep defaults and all
    /// ranges are clamped exactly like `load()`.
    pub fn parse(text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut cfg = Self::default();
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let key = k.trim();
            let raw_val = v.trim();
            // Macro rows hold a script where `;` is a separator, so an inline
            // comment rule would truncate `A=1 ; B=2`. Every other key keeps
            // the usual "whitespace then #/;" comment behaviour.
            let val = if matches!(key, "macro" | "macros") {
                raw_val
            } else {
                match inline_comment_start(raw_val) {
                    Some(index) => raw_val[..index].trim(),
                    None => raw_val,
                }
            };
            match key {
                "x" => {
                    if let Ok(n) = val.parse::<i32>() {
                        cfg.x = n;
                    }
                }
                "y" => {
                    if let Ok(n) = val.parse::<i32>() {
                        cfg.y = n;
                    }
                }
                "size" => {
                    if let Ok(n) = val.parse::<u32>() {
                        cfg.size = n;
                    }
                }
                "opacity" => {
                    if let Ok(n) = val.parse::<u32>() {
                        cfg.opacity = n;
                    }
                }
                "locked" => cfg.locked = parse_bool(val, cfg.locked),
                "always_on_top" => cfg.always_on_top = parse_bool(val, cfg.always_on_top),
                "overlay_visible" | "show_overlay" => {
                    cfg.overlay_visible = parse_bool(val, cfg.overlay_visible)
                }
                "macro" | "macros" => {
                    if let Some(mac) = Macro::parse(val) {
                        cfg.macros.push(mac);
                    }
                }
                "macro_capture" => cfg.macro_capture = val.trim().to_string(),
                "vm_mode" | "action_mode" => cfg.vm_mode = parse_vm_mode(val),
                "vm_param" => {
                    if !val.is_empty() {
                        cfg.vm_param = val.to_string();
                    }
                }
                "vm_param_b" | "vm_param2" | "vm_param_second" => {
                    if !val.is_empty() {
                        cfg.vm_param_b = val.to_string();
                    }
                }
                "invert" => cfg.invert = parse_bool(val, cfg.invert),
                "poll_ms" => {
                    if let Ok(n) = val.parse::<u32>() {
                        cfg.poll_ms = n;
                    }
                }
                "toggle_key" => {
                    let trigger = parse_trigger(val);
                    cfg.toggle_key = trigger.key;
                    cfg.toggle_mods = trigger.mods;
                }
                "trigger_mode" => cfg.trigger_mode = parse_trigger_mode(val),
                "hold_tap_ms" | "hold_ms" | "tap_ms" => {
                    if let Ok(n) = val.parse::<u32>() {
                        cfg.hold_tap_ms = n;
                    }
                }
                "visual_style" | "visual" | "style" => cfg.visual_style = parse_visual_style(val),
                "bg_opacity" | "bg_alpha" | "background_opacity" => {
                    if let Ok(n) = val.parse::<u32>() {
                        cfg.bg_opacity = n;
                    }
                }
                "speak_threshold_db" | "speak_threshold" => {
                    if let Ok(n) = val.parse::<i32>() {
                        cfg.speak_threshold_db = n;
                    }
                }
                "speak_release_ms" | "speak_release" => {
                    if let Ok(n) = val.parse::<u32>() {
                        cfg.speak_release_ms = n;
                    }
                }
                "anim_fps" | "wave_fps" => {
                    if let Ok(n) = val.parse::<u32>() {
                        cfg.anim_fps = n;
                    }
                }
                "wave_bars" | "bars" => {
                    if let Ok(n) = val.parse::<u32>() {
                        cfg.wave_bars = n;
                    }
                }
                "level_type" => {
                    if let Ok(n) = val.parse::<u32>() {
                        cfg.level_type = n;
                    }
                }
                "level_channel" => cfg.level_channel = parse_level_channel(val),
                "menu_style" | "tray_menu_style" | "menu" => cfg.menu_style = parse_menu_style(val),
                "color_on" => {
                    if let Some(c) = parse_hex6(val) {
                        cfg.color_on = c;
                    }
                }
                "color_off" => {
                    if let Some(c) = parse_hex6(val) {
                        cfg.color_off = c;
                    }
                }
                "color_unknown" => {
                    if let Some(c) = parse_hex6(val) {
                        cfg.color_unknown = c;
                    }
                }
                "color_bg" => {
                    if let Some(c) = parse_hex6(val) {
                        cfg.color_bg = c;
                    }
                }
                // Unknown keys ignored (includes legacy macro_* from v0.3.0).
                _ => {}
            }
        }
        cfg.clamp();
        cfg
    }

    pub fn toggle_trigger(&self) -> Trigger {
        Trigger {
            key: self.toggle_key,
            mods: self.toggle_mods,
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.write_full(&Self::config_path())
    }

    /// Full rewrite used only when creating a config from defaults. Every other
    /// write must go through `save_keys`/`update_config_file` so comments and
    /// unknown keys survive.
    fn write_full(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let _lock = ConfigLock::acquire(path)?;
        let mut content = format!(
            "# OnAir config (key=value)\n\
             x={}\n\
             y={}\n\
             size={}\n\
             opacity={}\n\
             locked={}\n\
             always_on_top={}\n\
             overlay_visible={}\n\
             vm_mode={}\n\
             vm_param={}\n\
             vm_param_b={}\n\
             invert={}\n\
             poll_ms={}\n\
             toggle_key={}\n\
             trigger_mode={}\n\
             hold_tap_ms={}\n\
             visual_style={}\n\
             bg_opacity={}\n\
             speak_threshold_db={}\n\
             speak_release_ms={}\n\
             anim_fps={}\n\
             wave_bars={}\n\
             level_type={}\n\
             level_channel={}\n\
             menu_style={}\n\
             color_on={:02X}{:02X}{:02X}\n\
             color_off={:02X}{:02X}{:02X}\n\
             color_unknown={:02X}{:02X}{:02X}\n\
             color_bg={:02X}{:02X}{:02X}\n",
            self.x,
            self.y,
            self.size,
            self.opacity,
            self.locked,
            self.always_on_top,
            self.overlay_visible,
            self.vm_mode.as_str(),
            self.vm_param,
            self.vm_param_b,
            self.invert,
            self.poll_ms,
            self.toggle_trigger().name(),
            self.trigger_mode.as_str(),
            self.hold_tap_ms,
            self.visual_style.as_str(),
            self.bg_opacity,
            self.speak_threshold_db,
            self.speak_release_ms,
            self.anim_fps,
            self.wave_bars,
            self.level_type,
            format_level_channel(self.level_channel),
            self.menu_style.as_str(),
            self.color_on.0,
            self.color_on.1,
            self.color_on.2,
            self.color_off.0,
            self.color_off.1,
            self.color_off.2,
            self.color_unknown.0,
            self.color_unknown.1,
            self.color_unknown.2,
            self.color_bg.0,
            self.color_bg.1,
            self.color_bg.2,
        );
        for mac in &self.macros {
            content.push_str(&mac.encode());
            content.push('\n');
        }
        write_atomic(path, content.as_bytes())
    }

    /// Replace every `macro=` line with `macros`, keeping all other lines
    /// (comments, ordering, unrelated keys) intact. Macro rows are inherently
    /// multi-line, so they bypass `apply_edits`.
    pub fn write_macros(macros: &[Macro]) -> std::io::Result<()> {
        Self::write_macros_to(&Self::config_path(), macros)
    }

    pub fn write_macros_to(path: &Path, macros: &[Macro]) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let _lock = ConfigLock::acquire(path)?;
        let existing = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(err),
        };
        let (bom, body) = match existing.strip_prefix('\u{feff}') {
            Some(rest) => ("\u{feff}", rest),
            None => ("", existing.as_str()),
        };
        let newline = if body.contains("\r\n") { "\r\n" } else { "\n" };
        let mut out = String::with_capacity(body.len() + 64);
        out.push_str(bom);
        for chunk in body.split_inclusive('\n') {
            let (line, _ending) = split_line_ending(chunk);
            let is_macro = line
                .split_once('=')
                .map(|(name, _)| {
                    matches!(
                        name.trim().to_ascii_lowercase().as_str(),
                        "macro" | "macros"
                    )
                })
                .unwrap_or(false);
            if is_macro {
                // Drop the row; the new macro block is appended below.
                continue;
            }
            out.push_str(chunk);
        }
        if !out.ends_with('\n') && !out.is_empty() {
            out.push_str(newline);
        }
        if !macros.is_empty() && !out.is_empty() {
            out.push_str(newline);
        }
        for mac in macros {
            out.push_str(&mac.encode());
            out.push_str(newline);
        }
        write_atomic(path, out.as_bytes())
    }

    /// Persist only `keys`, leaving every other line (comments, unknown keys,
    /// ordering and legacy aliases) untouched. The overlay uses this for the
    /// values it owns (position, lock, topmost, captured hotkey) so it can
    /// never clobber settings edited by `onair-settings`.
    pub fn save_keys(&self, keys: &[ConfigKey]) -> std::io::Result<()> {
        self.save_keys_to(&Self::config_path(), keys)
    }

    fn save_keys_to(&self, path: &Path, keys: &[ConfigKey]) -> std::io::Result<()> {
        let edits: Vec<(ConfigKey, String)> =
            keys.iter().map(|&key| (key, self.value_for(key))).collect();
        update_config_file(path, &edits)
    }

    fn value_for(&self, key: ConfigKey) -> String {
        match key {
            ConfigKey::X => self.x.to_string(),
            ConfigKey::Y => self.y.to_string(),
            ConfigKey::Size => self.size.to_string(),
            ConfigKey::Opacity => self.opacity.to_string(),
            ConfigKey::Locked => self.locked.to_string(),
            ConfigKey::AlwaysOnTop => self.always_on_top.to_string(),
            ConfigKey::OverlayVisible => self.overlay_visible.to_string(),
            ConfigKey::MacroCapture => self.macro_capture.clone(),
            ConfigKey::VmMode => self.vm_mode.as_str().to_string(),
            ConfigKey::VmParam => self.vm_param.clone(),
            ConfigKey::VmParamB => self.vm_param_b.clone(),
            ConfigKey::Invert => self.invert.to_string(),
            ConfigKey::PollMs => self.poll_ms.to_string(),
            ConfigKey::ToggleKey => self.toggle_trigger().name(),
            ConfigKey::TriggerMode => self.trigger_mode.as_str().to_string(),
            ConfigKey::HoldTapMs => self.hold_tap_ms.to_string(),
            ConfigKey::VisualStyle => self.visual_style.as_str().to_string(),
            ConfigKey::BgOpacity => self.bg_opacity.to_string(),
            ConfigKey::SpeakThresholdDb => self.speak_threshold_db.to_string(),
            ConfigKey::SpeakReleaseMs => self.speak_release_ms.to_string(),
            ConfigKey::AnimFps => self.anim_fps.to_string(),
            ConfigKey::WaveBars => self.wave_bars.to_string(),
            ConfigKey::LevelType => self.level_type.to_string(),
            ConfigKey::LevelChannel => format_level_channel(self.level_channel),
            ConfigKey::MenuStyle => self.menu_style.as_str().to_string(),
            ConfigKey::ColorOn => color_hex(self.color_on),
            ConfigKey::ColorOff => color_hex(self.color_off),
            ConfigKey::ColorUnknown => color_hex(self.color_unknown),
            ConfigKey::ColorBg => color_hex(self.color_bg),
        }
    }
}

fn color_hex(rgb: (u8, u8, u8)) -> String {
    format!("{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2)
}

/// Cross-process advisory lock around config read-modify-write. The overlay
/// and onair-settings both take it, so their line-preserving updates are
/// serialised instead of racing. The lock is a sibling file created with
/// `create_new`; a crashed owner's lock goes stale after 15 s.
struct ConfigLock {
    path: PathBuf,
}

impl ConfigLock {
    fn acquire(target: &Path) -> std::io::Result<Self> {
        use std::time::{Duration, Instant};

        const RETRY: Duration = Duration::from_millis(20);
        const TIMEOUT: Duration = Duration::from_secs(3);
        const STALE: Duration = Duration::from_secs(15);

        let path = target.with_extension("ini.lock");
        let start = Instant::now();
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let _ = write!(file, "{}", std::process::id());
                    return Ok(Self { path });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|meta| meta.modified())
                        .map(|time| time.elapsed().map(|age| age > STALE).unwrap_or(false))
                        .unwrap_or(false);
                    if stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if start.elapsed() >= TIMEOUT {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::WouldBlock,
                            "config.ini is busy",
                        ));
                    }
                    std::thread::sleep(RETRY);
                }
                Err(err) => return Err(err),
            }
        }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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
        rename_with_retry(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// `rename` can fail transiently on Windows while an antivirus, indexer or
/// editor briefly holds the destination open. Retry those cases; anything
/// else is a real failure.
fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(err) if attempt < 5 && is_transient_sharing_error(&err) => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(25 * attempt));
            }
            Err(err) => return Err(err),
        }
    }
}

fn is_transient_sharing_error(err: &std::io::Error) -> bool {
    // ERROR_ACCESS_DENIED (5), ERROR_SHARING_VIOLATION (32) and
    // ERROR_LOCK_VIOLATION (33) are the transient Windows cases.
    err.kind() == std::io::ErrorKind::PermissionDenied
        || matches!(err.raw_os_error(), Some(5) | Some(32) | Some(33))
}

/// Byte offset of a trailing inline comment (`#` or `;` preceded by
/// whitespace). Requiring whitespace keeps values such as `#2ECC71` intact.
fn inline_comment_start(text: &str) -> Option<usize> {
    let mut prev_ws = false;
    for (index, ch) in text.char_indices() {
        if (ch == '#' || ch == ';') && prev_ws {
            return Some(index);
        }
        prev_ws = ch.is_whitespace();
    }
    None
}

fn parse_bool(v: &str, fallback: bool) -> bool {
    match v.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => true,
        "false" | "0" | "no" | "off" => false,
        _ => fallback,
    }
}

/// `level_channel=auto` (or empty/unknown) means "follow vm_param". A number
/// selects the left channel explicitly; the right channel is always +1.
pub fn parse_level_channel(v: &str) -> Option<u32> {
    let s = v.trim().to_ascii_lowercase();
    if s.is_empty() || matches!(s.as_str(), "auto" | "default" | "none" | "off") {
        return None;
    }
    s.parse::<u32>().ok()
}

pub fn format_level_channel(channel: Option<u32>) -> String {
    match channel {
        Some(v) => v.to_string(),
        None => "auto".to_string(),
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

/// Voicemeeter button names accepted by the source dropdowns, in display
/// order. `Mute` is the only non-strip-flag entry.
pub const VM_BUTTONS: [&str; 9] = ["A1", "A2", "A3", "A4", "A5", "B1", "B2", "B3", "Mute"];

/// Split a canonical Voicemeeter parameter such as `Strip[0].B2` into a
/// 1-based strip number and a known button name. Returns `None` for custom
/// parameters, which the settings UI must preserve verbatim.
pub fn parse_strip_button(param: &str) -> Option<(u32, &str)> {
    let p = param.trim();
    let rest = p.strip_prefix("Strip[")?;
    let end = rest.find(']')?;
    let idx: u32 = rest[..end].parse().ok()?;
    if idx > 7 {
        return None;
    }
    let btn = rest[end + 1..].strip_prefix('.')?;
    if VM_BUTTONS.contains(&btn) {
        Some((idx + 1, btn))
    } else {
        None
    }
}

/// Build `Strip[n-1].Button` from a 1-based strip and a known button.
pub fn compose_vm_param(strip: u32, button: &str) -> Option<String> {
    if !(1..=8).contains(&strip) || !VM_BUTTONS.contains(&button) {
        return None;
    }
    Some(format!("Strip[{}].{}", strip - 1, button))
}

/// Loose validation for the editable raw `vm_param` field. Accepts
/// Voicemeeter-style `Name[index].Field` parameters while rejecting empty,
/// whitespace-containing or quote/equals-containing input.
pub fn looks_like_vm_param(param: &str) -> bool {
    let p = param.trim();
    if p.is_empty() || p.len() > 64 || !p.is_ascii() {
        return false;
    }
    if p.chars()
        .any(|c| c.is_whitespace() || c == '=' || c == '"' || c == '\'')
    {
        return false;
    }
    let Some(open) = p.find('[') else {
        return false;
    };
    let Some(close_rel) = p[open + 1..].find(']') else {
        return false;
    };
    let close = open + 1 + close_rel;
    let name = &p[..open];
    let idx = &p[open + 1..close];
    let rest = &p[close + 1..];
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !idx.is_empty()
        && idx.chars().all(|c| c.is_ascii_digit())
        && rest.len() > 1
        && rest.starts_with('.')
        && rest[1..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Every key the config file understands. Used by the line-preserving
/// updater so settings can write only the keys it owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigKey {
    X,
    Y,
    Size,
    Opacity,
    Locked,
    AlwaysOnTop,
    OverlayVisible,
    MacroCapture,
    VmMode,
    VmParam,
    VmParamB,
    Invert,
    PollMs,
    ToggleKey,
    TriggerMode,
    HoldTapMs,
    VisualStyle,
    BgOpacity,
    SpeakThresholdDb,
    SpeakReleaseMs,
    AnimFps,
    WaveBars,
    LevelType,
    LevelChannel,
    MenuStyle,
    ColorOn,
    ColorOff,
    ColorUnknown,
    ColorBg,
}

const ACTION_MODE_ALIASES: &[&str] = &["action_mode"];
const VM_PARAM_B_ALIASES: &[&str] = &["vm_param2", "vm_param_second"];
const HOLD_TAP_ALIASES: &[&str] = &["hold_ms", "tap_ms"];
const VISUAL_STYLE_ALIASES: &[&str] = &["visual", "style"];
const BG_OPACITY_ALIASES: &[&str] = &["bg_alpha", "background_opacity"];
const SPEAK_THRESHOLD_ALIASES: &[&str] = &["speak_threshold"];
const SPEAK_RELEASE_ALIASES: &[&str] = &["speak_release"];
const ANIM_FPS_ALIASES: &[&str] = &["wave_fps"];
const WAVE_BARS_ALIASES: &[&str] = &["bars"];
const MENU_STYLE_ALIASES: &[&str] = &["tray_menu_style", "menu"];

impl ConfigKey {
    pub fn name(self) -> &'static str {
        match self {
            ConfigKey::X => "x",
            ConfigKey::Y => "y",
            ConfigKey::Size => "size",
            ConfigKey::Opacity => "opacity",
            ConfigKey::Locked => "locked",
            ConfigKey::AlwaysOnTop => "always_on_top",
            ConfigKey::OverlayVisible => "overlay_visible",
            ConfigKey::MacroCapture => "macro_capture",
            ConfigKey::VmMode => "vm_mode",
            ConfigKey::VmParam => "vm_param",
            ConfigKey::VmParamB => "vm_param_b",
            ConfigKey::Invert => "invert",
            ConfigKey::PollMs => "poll_ms",
            ConfigKey::ToggleKey => "toggle_key",
            ConfigKey::TriggerMode => "trigger_mode",
            ConfigKey::HoldTapMs => "hold_tap_ms",
            ConfigKey::VisualStyle => "visual_style",
            ConfigKey::BgOpacity => "bg_opacity",
            ConfigKey::SpeakThresholdDb => "speak_threshold_db",
            ConfigKey::SpeakReleaseMs => "speak_release_ms",
            ConfigKey::AnimFps => "anim_fps",
            ConfigKey::WaveBars => "wave_bars",
            ConfigKey::LevelType => "level_type",
            ConfigKey::LevelChannel => "level_channel",
            ConfigKey::MenuStyle => "menu_style",
            ConfigKey::ColorOn => "color_on",
            ConfigKey::ColorOff => "color_off",
            ConfigKey::ColorUnknown => "color_unknown",
            ConfigKey::ColorBg => "color_bg",
        }
    }

    /// Legacy names accepted by `AppConfig::parse()`.
    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            ConfigKey::VmMode => ACTION_MODE_ALIASES,
            ConfigKey::VmParamB => VM_PARAM_B_ALIASES,
            ConfigKey::HoldTapMs => HOLD_TAP_ALIASES,
            ConfigKey::VisualStyle => VISUAL_STYLE_ALIASES,
            ConfigKey::BgOpacity => BG_OPACITY_ALIASES,
            ConfigKey::SpeakThresholdDb => SPEAK_THRESHOLD_ALIASES,
            ConfigKey::SpeakReleaseMs => SPEAK_RELEASE_ALIASES,
            ConfigKey::AnimFps => ANIM_FPS_ALIASES,
            ConfigKey::WaveBars => WAVE_BARS_ALIASES,
            ConfigKey::MenuStyle => MENU_STYLE_ALIASES,
            _ => &[],
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        const ALL: [ConfigKey; 29] = [
            ConfigKey::X,
            ConfigKey::Y,
            ConfigKey::Size,
            ConfigKey::Opacity,
            ConfigKey::Locked,
            ConfigKey::AlwaysOnTop,
            ConfigKey::OverlayVisible,
            ConfigKey::MacroCapture,
            ConfigKey::VmMode,
            ConfigKey::VmParam,
            ConfigKey::VmParamB,
            ConfigKey::Invert,
            ConfigKey::PollMs,
            ConfigKey::ToggleKey,
            ConfigKey::TriggerMode,
            ConfigKey::HoldTapMs,
            ConfigKey::VisualStyle,
            ConfigKey::BgOpacity,
            ConfigKey::SpeakThresholdDb,
            ConfigKey::SpeakReleaseMs,
            ConfigKey::AnimFps,
            ConfigKey::WaveBars,
            ConfigKey::LevelType,
            ConfigKey::LevelChannel,
            ConfigKey::MenuStyle,
            ConfigKey::ColorOn,
            ConfigKey::ColorOff,
            ConfigKey::ColorUnknown,
            ConfigKey::ColorBg,
        ];
        ALL.into_iter()
            .find(|k| k.name() == name || k.aliases().iter().any(|alias| *alias == name))
    }
}

/// Apply value edits to config text while keeping comments, unknown keys,
/// ordering and line endings. Existing canonical or alias lines are updated
/// in place; missing keys are appended. Duplicate edits for one key use the
/// last value.
pub fn apply_edits(text: &str, edits: &[(ConfigKey, String)]) -> String {
    let mut normalized: Vec<(ConfigKey, String)> = Vec::new();
    for (key, value) in edits {
        if let Some(slot) = normalized.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value.clone();
        } else {
            normalized.push((*key, value.clone()));
        }
    }

    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut result = String::with_capacity(text.len() + 64);
    let mut written = vec![false; normalized.len()];

    for chunk in text.split_inclusive('\n') {
        let (body, ending) = split_line_ending(chunk);
        let mut replaced = false;
        if let Some(eq) = body.find('=') {
            if let Some(line_key) = ConfigKey::from_name(&body[..eq]) {
                if let Some((idx, (_, value))) = normalized
                    .iter()
                    .enumerate()
                    .find(|(_, (k, _))| *k == line_key)
                {
                    let after = &body[eq + 1..];
                    let leading_ws = after.len() - after.trim_start_matches([' ', '\t']).len();
                    // Keep a trailing inline comment while replacing the value.
                    // Start after the leading whitespace: `key = #141414` has a
                    // value that begin with `#`, not a comment.
                    let rest = &after[leading_ws..];
                    let (value_part, comment) = match inline_comment_start(rest) {
                        Some(index) => (
                            &after[..leading_ws + index],
                            Some(&after[leading_ws + index..]),
                        ),
                        None => (after, None),
                    };
                    let trailing_ws = &value_part[value_part.trim_end_matches([' ', '\t']).len()..];
                    result.push_str(&body[..eq + 1 + leading_ws]);
                    result.push_str(value);
                    if let Some(comment) = comment {
                        result.push_str(trailing_ws);
                        result.push_str(comment);
                    }
                    result.push_str(ending);
                    written[idx] = true;
                    replaced = true;
                }
            }
        }
        if !replaced {
            result.push_str(chunk);
        }
    }

    for (idx, (key, value)) in normalized.iter().enumerate() {
        if written[idx] {
            continue;
        }
        if !result.is_empty() && !result.ends_with('\n') {
            result.push_str(newline);
        }
        result.push_str(key.name());
        result.push('=');
        result.push_str(value);
        result.push_str(newline);
    }
    result
}

fn split_line_ending(chunk: &str) -> (&str, &str) {
    if let Some(body) = chunk.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = chunk.strip_suffix('\n') {
        (body, "\n")
    } else {
        (chunk, "")
    }
}

/// Re-read `path` immediately before writing, replace only `edits`, and
/// atomically swap the file. Values the caller does not list (x/y/locked when
/// the overlay owns them) are never touched.
pub fn update_config_file(path: &Path, edits: &[(ConfigKey, String)]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let _lock = ConfigLock::acquire(path)?;
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    let (bom, body) = match existing.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", existing.as_str()),
    };
    let mut out = String::with_capacity(existing.len() + 64);
    out.push_str(bom);
    out.push_str(&apply_edits(body, edits));

    write_atomic(path, out.as_bytes())
}

fn exe_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|p| p.to_path_buf())
}

fn can_write_to_dir(dir: &Path) -> bool {
    // Try creating (and removing) a temp file. If that fails, dir is not writable.
    // Also treat "config.ini already exists and is writable" via same probe.
    let probe = dir.join(".vm-mic-write-test.tmp");
    match std::fs::write(&probe, b"test") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
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
        assert_eq!(
            parse_toggle_key("MOUSE_LEFT"),
            ToggleKey::Mouse(MouseBtn::Left)
        );
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
    fn vm_mode_parse() {
        assert_eq!(parse_vm_mode("toggle"), VmMode::Toggle);
        assert_eq!(parse_vm_mode("ON/OFF"), VmMode::Toggle);
        assert_eq!(parse_vm_mode("switch"), VmMode::Switch);
        assert_eq!(parse_vm_mode("SWAP"), VmMode::Switch);
        assert_eq!(parse_vm_mode("a/b"), VmMode::Switch);
        assert_eq!(parse_vm_mode("two buttons"), VmMode::Switch);
        assert_eq!(parse_vm_mode(" alternate "), VmMode::Switch);
        assert_eq!(parse_vm_mode(""), VmMode::Toggle);
        assert_eq!(parse_vm_mode("garbage"), VmMode::Toggle);
        assert_eq!(VmMode::Toggle.as_str(), "toggle");
        assert_eq!(VmMode::Switch.as_str(), "switch");
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
            vm_param_b: "  ".to_string(),
            ..Default::default()
        };
        cfg.clamp();
        assert_eq!(cfg.size, 16);
        assert_eq!(cfg.opacity, 20);
        assert_eq!(cfg.poll_ms, 30);
        assert_eq!(cfg.hold_tap_ms, 100);
        assert_eq!(cfg.bg_opacity, 100);
        assert_eq!(cfg.vm_param, "Strip[0].B2");
        assert_eq!(cfg.vm_param_b, "Strip[0].B1");

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
        assert_eq!(parse_visual_style("wave"), VisualStyle::Wave);
        assert_eq!(parse_visual_style("VOICE"), VisualStyle::Wave);
        assert_eq!(parse_visual_style("speaking"), VisualStyle::Wave);
        assert_eq!(parse_visual_style(""), VisualStyle::Dot);
        assert_eq!(parse_visual_style("garbage"), VisualStyle::Dot);
        assert_eq!(VisualStyle::Dot.as_str(), "dot");
        assert_eq!(VisualStyle::Ring.as_str(), "ring");
        assert_eq!(VisualStyle::Bar.as_str(), "bar");
        assert_eq!(VisualStyle::Mini.as_str(), "mini");
        assert_eq!(VisualStyle::Wave.as_str(), "wave");
    }

    #[test]
    fn menu_style_parse() {
        assert_eq!(parse_menu_style("glass"), MenuStyle::Glass);
        assert_eq!(parse_menu_style("acrylic"), MenuStyle::Glass);
        assert_eq!(parse_menu_style("native"), MenuStyle::Native);
        assert_eq!(parse_menu_style("CLASSIC"), MenuStyle::Native);
        assert_eq!(parse_menu_style("system"), MenuStyle::Native);
        // Unknown values and the default alias never fail.
        assert_eq!(parse_menu_style("default"), MenuStyle::Glass);
        assert_eq!(parse_menu_style(""), MenuStyle::Glass);
        assert_eq!(parse_menu_style("garbage"), MenuStyle::Glass);
        assert_eq!(MenuStyle::Glass.as_str(), "glass");
        assert_eq!(MenuStyle::Native.as_str(), "native");
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
    fn parse_supports_inline_comments() {
        let cfg =
            AppConfig::parse("size = 48 # pixels\ncolor_bg=#141414\ntoggle_key=None ; no bind\n");
        assert_eq!(cfg.size, 48);
        // A '#' that starts the value is not a comment.
        assert_eq!(cfg.color_bg, (0x14, 0x14, 0x14));
        assert!(cfg.toggle_key.is_none());
    }

    #[test]
    fn update_preserves_inline_comments() {
        let text = "size = 48 # pixels\ncolor_on = 2ECC71 ; green\n";
        let out = apply_edits(text, &[(ConfigKey::Size, "64".into())]);
        assert_eq!(out, "size = 64 # pixels\ncolor_on = 2ECC71 ; green\n");

        // A value that starts with '#' is not an inline comment.
        let text = "color_bg = #141414\n";
        let out = apply_edits(text, &[(ConfigKey::ColorBg, "202020".into())]);
        assert_eq!(out, "color_bg = 202020\n");
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

    fn temp_config_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!(
                "onair-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
            .join("config.ini")
    }

    #[test]
    fn load_missing_file_creates_defaults() {
        let path = temp_config_path("load-missing");
        let cfg = AppConfig::load_from(&path);
        assert_eq!(cfg.size, AppConfig::default().size);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("size="));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn load_read_error_does_not_overwrite_config() {
        let path = temp_config_path("load-read-error");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Invalid UTF-8 makes read_to_string fail with InvalidData, not NotFound.
        let original = [0xFFu8, 0xFE, 0x00, 0x01];
        std::fs::write(&path, original).unwrap();

        let cfg = AppConfig::load_from(&path);
        assert_eq!(cfg.size, AppConfig::default().size);
        assert_eq!(std::fs::read(&path).unwrap(), original);

        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn config_lock_is_released_on_drop() {
        let path = temp_config_path("lock");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let _lock = ConfigLock::acquire(&path).unwrap();
            assert!(path.with_extension("ini.lock").exists());
        }
        // A second acquire must succeed once the first lock is dropped.
        let _lock = ConfigLock::acquire(&path).unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn save_keys_preserves_untouched_lines() {
        let path = temp_config_path("save-keys");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "# keep me\nx=1\ny=2\ncolor_on=000000\nunknown=keep\n",
        )
        .unwrap();

        let cfg = AppConfig {
            x: 10,
            y: 20,
            ..AppConfig::default()
        };
        cfg.save_keys_to(&path, &[ConfigKey::X, ConfigKey::Y])
            .unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep me"));
        assert!(text.contains("unknown=keep"));
        assert!(text.contains("x=10"));
        assert!(text.contains("y=20"));
        // Settings-owned values must survive an overlay save untouched.
        assert!(text.contains("color_on=000000"));

        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn bg_opacity_clamp() {
        let mut cfg = AppConfig::default();
        assert_eq!(cfg.bg_opacity, 90);
        cfg.bg_opacity = 500;
        cfg.clamp();
        assert_eq!(cfg.bg_opacity, 100);
    }

    #[test]
    fn wave_defaults_and_clamps() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.speak_threshold_db, -45);
        assert_eq!(cfg.speak_release_ms, 300);
        assert_eq!(cfg.anim_fps, 30);
        assert_eq!(cfg.wave_bars, 5);
        assert_eq!(cfg.level_type, 0);
        assert_eq!(cfg.level_channel, None);

        let mut cfg = AppConfig {
            speak_threshold_db: -100,
            speak_release_ms: 0,
            anim_fps: 999,
            wave_bars: 1,
            level_type: 9,
            level_channel: Some(99_999),
            ..Default::default()
        };
        cfg.clamp();
        assert_eq!(cfg.speak_threshold_db, -70);
        assert_eq!(cfg.speak_release_ms, 100);
        assert_eq!(cfg.anim_fps, 60);
        assert_eq!(cfg.wave_bars, 3);
        assert_eq!(cfg.level_type, 3);
        assert_eq!(cfg.level_channel, Some(4095));

        let mut cfg = AppConfig {
            speak_threshold_db: 0,
            speak_release_ms: 99_999,
            anim_fps: 0,
            wave_bars: 99,
            ..Default::default()
        };
        cfg.clamp();
        assert_eq!(cfg.speak_threshold_db, -20);
        assert_eq!(cfg.speak_release_ms, 1500);
        assert_eq!(cfg.anim_fps, 10);
        assert_eq!(cfg.wave_bars, 9);
    }

    #[test]
    fn wave_keys_parse_aliases_and_invalid_values_fall_back() {
        let cfg = AppConfig::parse(
            "visual_style=voice\n\
             speak_threshold=-55\n\
             speak_release_ms=500\n\
             wave_fps=45\n\
             bars=7\n\
             level_type=2\n\
             level_channel=8\n",
        );
        assert_eq!(cfg.visual_style, VisualStyle::Wave);
        assert_eq!(cfg.speak_threshold_db, -55);
        assert_eq!(cfg.speak_release_ms, 500);
        assert_eq!(cfg.anim_fps, 45);
        assert_eq!(cfg.wave_bars, 7);
        assert_eq!(cfg.level_type, 2);
        assert_eq!(cfg.level_channel, Some(8));

        // Unknown / malformed values keep the defaults instead of failing.
        let cfg = AppConfig::parse(
            "speak_threshold_db=loud\n\
             speak_release_ms=-2\n\
             anim_fps=fast\n\
             wave_bars=\n\
             level_type=x\n\
             level_channel=left\n",
        );
        assert_eq!(cfg.speak_threshold_db, -45);
        assert_eq!(cfg.speak_release_ms, 300);
        assert_eq!(cfg.anim_fps, 30);
        assert_eq!(cfg.wave_bars, 5);
        assert_eq!(cfg.level_type, 0);
        assert_eq!(cfg.level_channel, None);
    }

    #[test]
    fn level_channel_auto_and_number_roundtrip() {
        assert_eq!(parse_level_channel("auto"), None);
        assert_eq!(parse_level_channel("AUTO"), None);
        assert_eq!(parse_level_channel("none"), None);
        assert_eq!(parse_level_channel(""), None);
        assert_eq!(parse_level_channel(" 4 "), Some(4));
        assert_eq!(parse_level_channel("left"), None);
        assert_eq!(format_level_channel(None), "auto");
        assert_eq!(format_level_channel(Some(6)), "6");

        let mut cfg = AppConfig {
            level_channel: Some(12),
            ..Default::default()
        };
        cfg.macros.clear();
        let path = temp_config_path("wave-roundtrip");
        cfg.write_full(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("level_channel=12\n"), "text was: {text}");
        assert!(text.contains("visual_style=dot\n"), "text was: {text}");
        let loaded = AppConfig::parse(&text);
        assert_eq!(loaded.level_channel, Some(12));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());

        let defaults = AppConfig::default();
        let path = temp_config_path("wave-auto");
        defaults.write_full(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("level_channel=auto\n"), "text was: {text}");
        assert_eq!(AppConfig::parse(&text).level_channel, None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn update_preserves_comments_unknown_keys_and_order() {
        let text = "# OnAir config\n\
                    ; keep me\n\
                    size = 48\n\
                    unknown_future_key = hello\n\
                    \n\
                    visual_style=dot\n";
        let out = apply_edits(
            text,
            &[
                (ConfigKey::Size, "64".to_string()),
                (ConfigKey::ColorOn, "2ECC71".to_string()),
            ],
        );
        assert_eq!(
            out,
            "# OnAir config\n\
             ; keep me\n\
             size = 64\n\
             unknown_future_key = hello\n\
             \n\
             visual_style=dot\n\
             color_on=2ECC71\n"
        );

        let cfg = AppConfig::parse(&out);
        assert_eq!(cfg.size, 64);
        assert_eq!(cfg.color_on, (0x2E, 0xCC, 0x71));
    }

    #[test]
    fn update_rewrites_aliases_in_place() {
        let text = "action_mode=toggle\nvm_param2=Strip[0].B1\ntray_menu_style=glass\n";
        let out = apply_edits(
            text,
            &[
                (ConfigKey::VmMode, "switch".to_string()),
                (ConfigKey::VmParamB, "Strip[2].A1".to_string()),
                (ConfigKey::MenuStyle, "native".to_string()),
            ],
        );
        assert_eq!(
            out,
            "action_mode=switch\nvm_param2=Strip[2].A1\ntray_menu_style=native\n"
        );
        let cfg = AppConfig::parse(&out);
        assert_eq!(cfg.vm_mode, VmMode::Switch);
        assert_eq!(cfg.vm_param_b, "Strip[2].A1");
        assert_eq!(cfg.menu_style, MenuStyle::Native);
    }

    #[test]
    fn update_rewrites_every_duplicate_alias_line() {
        let text = "vm_param2=one\nvm_param_second=two\nvm_param_b=three\n";
        let out = apply_edits(text, &[(ConfigKey::VmParamB, "Strip[1].B3".to_string())]);
        assert_eq!(
            out,
            "vm_param2=Strip[1].B3\nvm_param_second=Strip[1].B3\nvm_param_b=Strip[1].B3\n"
        );
    }

    #[test]
    fn update_preserves_crlf() {
        let text = "# c\r\nsize=48\r\n";
        let out = apply_edits(text, &[(ConfigKey::Size, "96".to_string())]);
        assert_eq!(out, "# c\r\nsize=96\r\n");
    }

    #[test]
    fn update_file_keeps_overlay_owned_position_and_lock() {
        let dir = std::env::temp_dir().join(format!(
            "onair-update-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.ini");
        std::fs::write(
            &path,
            "# comment\nx=111\ny=222\nlocked=true\nsize=48\nunknown=keep\n",
        )
        .unwrap();

        update_config_file(&path, &[(ConfigKey::Size, "96".to_string())]).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# comment"));
        assert!(text.contains("unknown=keep"));
        let cfg = AppConfig::parse(&text);
        assert_eq!(cfg.x, 111);
        assert_eq!(cfg.y, 222);
        assert!(cfg.locked);
        assert_eq!(cfg.size, 96);
        assert!(!dir.join("config.ini.tmp").exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn vm_param_dropdown_mapping_and_custom_values() {
        assert_eq!(parse_strip_button("Strip[0].B2"), Some((1, "B2")));
        assert_eq!(parse_strip_button("Strip[7].Mute"), Some((8, "Mute")));
        assert_eq!(compose_vm_param(1, "B2").as_deref(), Some("Strip[0].B2"));
        assert_eq!(compose_vm_param(8, "A5").as_deref(), Some("Strip[7].A5"));
        // Out of range, unknown button, and hand-edited custom values.
        assert_eq!(parse_strip_button("Strip[8].B2"), None);
        assert_eq!(parse_strip_button("Strip[0].Gain"), None);
        assert_eq!(parse_strip_button("Bus[0].Mute"), None);
        assert_eq!(compose_vm_param(0, "B2"), None);
        assert_eq!(compose_vm_param(1, "Gain"), None);
        assert!(looks_like_vm_param("Strip[0].B2"));
        assert!(looks_like_vm_param("Bus[1].Mute"));
        assert!(looks_like_vm_param("Strip[0].Gain"));
        assert!(!looks_like_vm_param(""));
        assert!(!looks_like_vm_param("Strip[0]"));
        assert!(!looks_like_vm_param("Strip[a].B2"));
        assert!(!looks_like_vm_param("Strip[0].B2 extra"));
        assert!(!looks_like_vm_param("Strip[0].B2=1"));
    }

    #[test]
    fn parse_strips_utf8_bom() {
        let cfg = AppConfig::parse("\u{feff}size=96\nvm_param_b=Strip[3].A2\n");
        assert_eq!(cfg.size, 96);
        assert_eq!(cfg.vm_param_b, "Strip[3].A2");
    }

    #[test]
    fn trigger_parse_and_name() {
        assert_eq!(parse_trigger("F13").name(), "F13");
        assert_eq!(parse_trigger("ctrl+alt+f13").name(), "Ctrl+Alt+F13");
        assert_eq!(parse_trigger("Shift+Mouse X1").name(), "Shift+Mouse X1");
        assert_eq!(
            parse_trigger("Alt+Ctrl+Shift+Win+A").name(),
            "Ctrl+Alt+Shift+Win+A"
        );
        assert_eq!(parse_trigger("Ctrl"), Trigger::NONE);
        assert_eq!(parse_trigger("Ctrl+Shift"), Trigger::NONE);
        assert_eq!(parse_trigger("Esc"), Trigger::NONE);
        assert_eq!(parse_trigger(""), Trigger::NONE);
        // Round trip through the canonical label.
        let t = parse_trigger("Ctrl+Win+B");
        assert_eq!(parse_trigger(&t.name()), t);
    }

    #[test]
    fn toggle_trigger_modifiers_roundtrip() {
        let cfg = AppConfig::parse("toggle_key=Ctrl+Alt+F13\n");
        assert_eq!(cfg.toggle_key, ToggleKey::Key(0x7C));
        assert!(cfg.toggle_mods.ctrl && cfg.toggle_mods.alt);
        assert_eq!(cfg.toggle_trigger().name(), "Ctrl+Alt+F13");
        assert_eq!(cfg.value_for(ConfigKey::ToggleKey), "Ctrl+Alt+F13");

        let cfg = AppConfig::parse("toggle_key=F13\n");
        assert!(cfg.toggle_mods.is_none());
        assert_eq!(cfg.toggle_trigger().name(), "F13");
    }

    #[test]
    fn macro_capture_line_roundtrip() {
        let cfg = AppConfig::parse("macro_capture=1|Ctrl+F13\n");
        assert_eq!(cfg.macro_capture, "1|Ctrl+F13");
        assert_eq!(cfg.value_for(ConfigKey::MacroCapture), "1|Ctrl+F13");
        let text = apply_edits(
            "macro_capture=1|Ctrl+F13\nsize=48\n",
            &[(ConfigKey::MacroCapture, String::new())],
        );
        assert!(text.contains("macro_capture=\n"));
        assert!(text.contains("size=48"));
    }

    #[test]
    fn macro_mode_parse() {
        assert_eq!(parse_macro_mode("hold"), MacroMode::Hold);
        assert_eq!(parse_macro_mode("Push-To-Talk"), MacroMode::Hold);
        assert_eq!(parse_macro_mode("both"), MacroMode::Both);
        assert_eq!(parse_macro_mode("press"), MacroMode::Press);
        assert_eq!(parse_macro_mode("garbage"), MacroMode::Press);
    }

    #[test]
    fn macro_encode_parse_roundtrip() {
        let mac = Macro {
            trigger: parse_trigger("Ctrl+Alt+F13"),
            mode: MacroMode::Hold,
            label: "Mic mute".to_string(),
            press: "Strip[0].B2=1; Strip[0].Gain+=1.5".to_string(),
            release: "Strip[0].B2=0".to_string(),
        };
        let encoded = mac.encode();
        assert_eq!(Macro::parse(&encoded), Some(mac));
        assert_eq!(
            Macro::parse("F13|press|Label|Strip[0].B2=!|")
                .unwrap()
                .release,
            ""
        );
        // Newlines in a script become `;` so the row stays one line and still
        // parses back into the same steps.
        let multiline = Macro {
            trigger: parse_trigger("F14"),
            mode: MacroMode::Press,
            label: "Two steps".to_string(),
            press: "Strip[0].B2=1\r\nStrip[0].Gain+=1.5".to_string(),
            release: String::new(),
        };
        let encoded = multiline.encode();
        assert!(!encoded.contains('\n'));
        let parsed = Macro::parse(&encoded).unwrap();
        assert_eq!(parse_macro_script(&parsed.press).unwrap().len(), 2);
        // Unusable rows are rejected.
        assert_eq!(Macro::parse("None|press|x|Strip[0].B2=1|"), None);
        assert_eq!(Macro::parse("F13|press|x||"), None);
        assert_eq!(Macro::parse("F13|press|x"), None);
    }

    #[test]
    fn macro_script_parse() {
        let steps =
            parse_macro_script("Strip[0].B2=1\n# comment\nBus[1].Gain+=1.5; Bus[1].Gain-=0.5")
                .unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].param, "Strip[0].B2");
        assert_eq!(steps[0].op, MacroOp::Set(1.0));
        assert_eq!(steps[1].op, MacroOp::Add(1.5));
        assert_eq!(steps[2].op, MacroOp::Sub(0.5));
        assert_eq!(
            parse_macro_script("Strip[0].B2=!").unwrap()[0].op,
            MacroOp::Toggle
        );
        assert!(parse_macro_script("").is_err());
        assert!(parse_macro_script("Strip[0].B2").is_err());
        assert!(parse_macro_script("Strip[0].B2=abc").is_err());
        assert!(parse_macro_script("Strip[0].B2=NaN").is_err());
        assert!(parse_macro_script("Bad Param=1").is_err());
    }

    #[test]
    fn macro_rows_keep_semicolon_separated_scripts() {
        // A space before `;` must not be treated as an inline comment.
        let cfg = AppConfig::parse("macro=F13|press|Gain|Strip[0].B2=1 ; Strip[0].Gain+=1.5|\n");
        assert_eq!(cfg.macros.len(), 1);
        let steps = parse_macro_script(&cfg.macros[0].press).unwrap();
        assert_eq!(steps.len(), 2);
        // Other keys still support inline comments.
        let cfg = AppConfig::parse("size=64 ; note\n");
        assert_eq!(cfg.size, 64);
    }

    #[test]
    fn duplicate_macro_triggers_are_dropped() {
        let cfg = AppConfig::parse(
            "macro=F13|press|a|Strip[0].B2=1|\nmacro=F13|press|b|Strip[0].B2=0|\nmacro=Ctrl+F14|hold|c|Strip[0].B1=1|\n",
        );
        assert_eq!(cfg.macros.len(), 2);
        assert_eq!(cfg.macros[0].label, "a");
        assert_eq!(cfg.macros[1].label, "c");

        // A macro may not reuse the overlay hotkey.
        let cfg = AppConfig::parse(
            "toggle_key=F13\nmacro=F13|press|same|Strip[0].B2=1|\nmacro=Ctrl+F13|press|ok|Strip[0].B2=0|\n",
        );
        assert_eq!(cfg.macros.len(), 1);
        assert_eq!(cfg.macros[0].label, "ok");
    }

    #[test]
    fn write_macros_replaces_rows_and_keeps_comments() {
        let dir = std::env::temp_dir().join(format!("onair-macros-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.ini");
        std::fs::write(
            &path,
            "# keep me\r\nmacro=F13|press|old|Strip[0].B2=1|\r\nsize=48\r\n",
        )
        .unwrap();
        let macros = vec![
            Macro {
                trigger: parse_trigger("Ctrl+F13"),
                mode: MacroMode::Press,
                label: "new".to_string(),
                press: "Strip[0].B2=1".to_string(),
                release: String::new(),
            },
            Macro {
                trigger: parse_trigger("Ctrl+F14"),
                mode: MacroMode::Hold,
                label: "second".to_string(),
                press: "Strip[0].B1=1".to_string(),
                release: String::new(),
            },
        ];
        AppConfig::write_macros_to(&path, &macros).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep me"));
        assert!(text.contains("size=48"));
        assert!(!text.contains("|old|"));
        assert_eq!(text.matches("macro=").count(), 2);
        assert!(text.contains("\r\n"));
        let cfg = AppConfig::parse(&text);
        assert_eq!(cfg.macros, macros);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
