//! HSV colour wheel helpers for the settings colour picker.
//!
//! Rust owns the HSV state; Slint receives everything it needs as plain
//! properties and only forwards pointer gestures back. The disc image is
//! generated once (176x176) and never changes; hue and saturation are what
//! the pointer picks, value is the separate vertical slider.

use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::AppWindow;

const DISC_SIZE: u32 = 176;
const DISC_CENTER: f32 = DISC_SIZE as f32 / 2.0;
const DISC_RADIUS: f32 = 84.0;
const VALUE_STRIP_PX: f32 = DISC_SIZE as f32;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ColorField {
    On,
    Off,
    Unknown,
    Background,
}

pub fn field_key(field: ColorField) -> &'static str {
    match field {
        ColorField::On => "color_on",
        ColorField::Off => "color_off",
        ColorField::Unknown => "color_unknown",
        ColorField::Background => "color_bg",
    }
}

pub fn field_title(field: ColorField) -> &'static str {
    match field {
        ColorField::On => "Colour — On",
        ColorField::Off => "Colour — Off",
        ColorField::Unknown => "Colour — Unknown",
        ColorField::Background => "Background colour",
    }
}

pub fn default_hex(field: ColorField) -> &'static str {
    match field {
        ColorField::On => "2ECC71",
        ColorField::Off => "E74C3C",
        ColorField::Unknown => "808080",
        ColorField::Background => "141414",
    }
}

pub fn parse_target(target: &str) -> Option<ColorField> {
    match target {
        "color_on" => Some(ColorField::On),
        "color_off" => Some(ColorField::Off),
        "color_unknown" => Some(ColorField::Unknown),
        "color_bg" => Some(ColorField::Background),
        _ => None,
    }
}

/// The canonical last-good colour for the field, read from the file
/// that is on disk right now (after any pending edit was flushed).
pub fn canonical_hex(field: ColorField) -> String {
    let cfg = onair_config::AppConfig::load();
    let (r, g, b) = match field {
        ColorField::On => cfg.color_on,
        ColorField::Off => cfg.color_off,
        ColorField::Unknown => cfg.color_unknown,
        ColorField::Background => cfg.color_bg,
    };
    hex_label(r, g, b)
}

/// Luminance-based contrast decision for text drawn on the preview swatch.
pub fn prefers_dark_text(red: u8, green: u8, blue: u8) -> bool {
    let r = f32::from(red) / 255.0;
    let g = f32::from(green) / 255.0;
    let b = f32::from(blue) / 255.0;
    let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    luminance < 0.42
}

pub fn rgb_to_hsv(red: u8, green: u8, blue: u8) -> (f32, f32, f32) {
    let r = f32::from(red) / 255.0;
    let g = f32::from(green) / 255.0;
    let b = f32::from(blue) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let hue = if delta == 0.0 {
        0.0
    } else if (max - r).abs() < f32::EPSILON {
        60.0 * (((g - b) / delta) % 6.0)
    } else if (max - g).abs() < f32::EPSILON {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    let hue = (hue + 360.0) % 360.0;
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    (hue, saturation, max)
}

pub fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> (u8, u8, u8) {
    let hue = ((hue % 360.0) + 360.0) % 360.0;
    let s = saturation.clamp(0.0, 1.0);
    let v = value.clamp(0.0, 1.0);
    let c = v * s;
    let hp = hue / 60.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match () {
        () if hp < 1.0 => (c, x, 0.0),
        () if hp < 2.0 => (x, c, 0.0),
        () if hp < 3.0 => (0.0, c, x),
        () if hp < 4.0 => (0.0, x, c),
        () if hp < 5.0 => (x, 0.0, c),
        () => (c, 0.0, x),
    };
    let m = v - c;
    let to_u8 = |channel: f32| ((channel + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (to_u8(r1), to_u8(g1), to_u8(b1))
}

pub fn hex_label(red: u8, green: u8, blue: u8) -> String {
    format!("{red:02X}{green:02X}{blue:02X}")
}

pub struct PickerState {
    pub field: ColorField,
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

impl PickerState {
    pub fn from_hex(field: ColorField, hex: &str) -> Option<Self> {
        let raw = hex.trim().trim_start_matches('#');
        let (r, g, b) = onair_config::parse_hex6(raw)?;
        let (h, s, v) = rgb_to_hsv(r, g, b);
        Some(Self { field, h, s, v })
    }

    pub fn reset(&self) -> Self {
        Self::from_hex(self.field, default_hex(self.field)).expect("built-in default is valid")
    }

    pub fn rgb(&self) -> (u8, u8, u8) {
        hsv_to_rgb(self.h, self.s, self.v)
    }

    pub fn hex(&self) -> String {
        let (r, g, b) = self.rgb();
        hex_label(r, g, b)
    }
}

/// Keep the Slint properties in sync with the picker state.
pub fn push_props(ui: &AppWindow, state: &PickerState) {
    let (r, g, b) = state.rgb();
    let (top_r, top_g, top_b) = hsv_to_rgb(state.h, state.s, 1.0);
    ui.set_picker_title(field_title(state.field).into());
    ui.set_picker_hex(state.hex().as_str().into());
    ui.set_picker_preview(slint::Color::from_rgb_u8(r, g, b));
    ui.set_picker_preview_dark(prefers_dark_text(r, g, b));
    ui.set_picker_value_top(slint::Color::from_rgb_u8(top_r, top_g, top_b));
    ui.set_picker_value(state.v);
    let (x, y) = marker_pos(state);
    ui.set_picker_marker_x(x);
    ui.set_picker_marker_y(y);
}

/// Normalised disc coordinates in -1..1 (x right, y down).
pub fn marker_pos(state: &PickerState) -> (f32, f32) {
    let hue = state.h.to_radians();
    let radius = state.s;
    (hue.cos() * radius, hue.sin() * radius)
}

/// Inverse of [`marker_pos`]: clamp the pointer to the disc and return
/// the hue/saturation for it. Distances beyond the radius saturate at
/// the edge instead of wrapping around.
pub fn pick_disc(x: f32, y: f32) -> (f32, f32) {
    let dx = x - DISC_CENTER;
    let dy = y - DISC_CENTER;
    let dist = (dx * dx + dy * dy).sqrt();
    let saturation = (dist / DISC_RADIUS).min(1.0);
    if dist < 0.5 {
        return (0.0, 0.0);
    }
    let mut hue = dy.atan2(dx).to_degrees();
    if hue < 0.0 {
        hue += 360.0;
    }
    (hue, saturation)
}

pub fn value_from_y(y: f32) -> f32 {
    1.0 - (y / VALUE_STRIP_PX).clamp(0.0, 1.0)
}

/// Rotate the hue, wrapping around the colour circle.
pub fn nudge_hue(state: &mut PickerState, degrees: f32) {
    state.h = (state.h + degrees).rem_euclid(360.0);
}

/// Move the marker towards or away from the centre of the disc.
pub fn nudge_saturation(state: &mut PickerState, delta: f32) {
    state.s = (state.s + delta).clamp(0.0, 1.0);
}

/// Move the brightness slider up or down.
pub fn nudge_value(state: &mut PickerState, delta: f32) {
    state.v = (state.v + delta).clamp(0.0, 1.0);
}

/// Render the hue/saturation disc once at startup.
pub fn disc_image() -> slint::Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(DISC_SIZE, DISC_SIZE);
    let pixels = buf.make_mut_slice();
    for py in 0..DISC_SIZE {
        for px in 0..DISC_SIZE {
            let dx = px as f32 + 0.5 - DISC_CENTER;
            let dy = py as f32 + 0.5 - DISC_CENTER;
            let dist = (dx * dx + dy * dy).sqrt();
            let idx = (py * DISC_SIZE + px) as usize;
            if dist > DISC_RADIUS {
                pixels[idx] = Rgba8Pixel::new(0, 0, 0, 0);
                continue;
            }
            let mut hue = dy.atan2(dx).to_degrees();
            if hue < 0.0 {
                hue += 360.0;
            }
            let saturation = (dist / DISC_RADIUS).min(1.0);
            let (r, g, b) = hsv_to_rgb(hue, saturation, 1.0);
            pixels[idx] = Rgba8Pixel::new(r, g, b, 255);
        }
    }
    slint::Image::from_rgba8(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nudges_wrap_and_clamp() {
        let mut state = PickerState {
            field: ColorField::On,
            h: 350.0,
            s: 0.5,
            v: 0.9,
        };
        nudge_hue(&mut state, 20.0);
        assert_eq!(state.h, 10.0);
        nudge_hue(&mut state, -20.0);
        assert_eq!(state.h, 350.0);

        nudge_saturation(&mut state, 1.0);
        assert_eq!(state.s, 1.0);
        nudge_saturation(&mut state, -2.0);
        assert_eq!(state.s, 0.0);

        nudge_value(&mut state, 0.5);
        assert_eq!(state.v, 1.0);
        nudge_value(&mut state, -2.0);
        assert_eq!(state.v, 0.0);
    }
}
