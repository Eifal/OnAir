use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::VisualStyle;

/// Wave parameters for one frame. `level` is the smoothed 0..1 level and
/// `phase` advances in seconds; `bars` is the configured bar count.
#[derive(Debug, Clone, Copy)]
pub struct WaveView {
    pub level: f32,
    pub phase: f32,
    pub bars: u32,
}

// Wave geometry as fractions of the overlay size. The inner square is the
// usable area inside the rounded background; the level math lives in
// `crate::wave` so it can be unit tested without Win32.

/// Draw overlay pixels (BGRA premultiplied, top-down) of `size`x`size`.
///
/// Styles (same square window, only drawing differs):
/// - Dot: rounded square bg + centered filled circle (diameter 80%)
/// - Ring: rounded square bg + colored ring (outer 80%, thickness ~10%)
/// - Bar: rounded square bg + thin colored strip at the bottom
/// - Mini: no bg, just a small filled dot (diameter ~54%)
/// - Wave: rounded square bg + vertical capsule bars driven by voice level
///
/// When `unlocked`, thin light outline around the rounded square.
/// SDF AA: coverage = clamp(0.5 - dist, 0, 1)
pub fn draw_overlay(
    size: u32,
    bg: (u8, u8, u8),
    dot: (u8, u8, u8),
    unlocked: bool,
    style: VisualStyle,
    bg_opacity: u32,
    wave: Option<WaveView>,
) -> Vec<u32> {
    let mut px = vec![0u32; (size as usize) * (size as usize)];
    draw_overlay_into(
        &mut px, size, bg, dot, unlocked, style, bg_opacity, wave, None,
    );
    px
}

/// Same as `draw_overlay`, but writes into a caller-owned buffer so the
/// animation can reuse one allocation for the lifetime of the process.
/// `mute` replaces the style glyph with a prohibition sign in that colour
/// (used while the Voicemeeter strip is muted).
#[allow(clippy::too_many_arguments)]
#[inline(never)]
pub fn draw_overlay_into(
    px: &mut [u32],
    size: u32,
    bg: (u8, u8, u8),
    dot: (u8, u8, u8),
    unlocked: bool,
    style: VisualStyle,
    bg_opacity: u32,
    wave: Option<WaveView>,
    mute: Option<(u8, u8, u8)>,
) -> bool {
    let needed = (size as usize) * (size as usize);
    if size == 0 || px.len() < needed {
        return false;
    }
    let s = size as f32;
    let half = s / 2.0;
    let radius = s * 0.22;
    // Background alpha 0..100% -> 0..255. Foreground (dot/ring/bar)
    // stays fully opaque; global `opacity` still applies on top via
    // SourceConstantAlpha in update_layered_window.
    let bg_alpha: f32 = (bg_opacity.clamp(0, 100) as f32 * 255.0 / 100.0).clamp(0.0, 255.0);

    let box_half = half - radius; // for rounded-rect SDF

    // Bar geometry (shared for Bar style): thin strip at the bottom.
    let bar_cx = half;
    let bar_cy = s * 0.765;
    let bar_hw = s * 0.35; // half width
    let bar_hh = s * 0.085; // half height
    let bar_r = (s * 0.06).min(bar_hw).min(bar_hh);

    // Wave bar geometry: even spacing with gap == bar width. Heights are
    // computed once per frame; the pixel loop only evaluates the SDFs.
    let bars = wave.map(|w| w.bars.clamp(3, 9)).unwrap_or(5) as usize;
    let inner_w = s * crate::wave::INNER_W;
    let inner_h = s * crate::wave::INNER_H;
    let bar_w = inner_w / (2.0 * bars as f32);
    let gap = bar_w;
    let total_w = bars as f32 * bar_w + (bars.saturating_sub(1)) as f32 * gap;
    let wave_x0 = half - total_w * 0.5 + bar_w * 0.5;
    let wave_step = bar_w + gap;
    let min_h = bar_w;
    let max_h = inner_h * crate::wave::MAX_H;
    let mut heights = [0.0f32; 9];
    if let Some(w) = wave {
        let level = w.level.clamp(0.0, 1.0);
        for (i, h) in heights.iter_mut().take(bars).enumerate() {
            *h = crate::wave::bar_height(i, level, w.phase, min_h, max_h);
        }
    } else {
        for h in heights.iter_mut().take(bars) {
            *h = min_h;
        }
    }

    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;

            // Rounded-rect SDF (negative inside)
            let qx = (fx - half).abs() - box_half;
            let qy = (fy - half).abs() - box_half;
            let ax = qx.max(0.0);
            let ay = qy.max(0.0);
            let outside = (ax * ax + ay * ay).sqrt();
            let inside = qx.max(qy).min(0.0);
            let dist_rr = outside + inside - radius;
            let cov_bg = (0.5 - dist_rr).clamp(0.0, 1.0);

            let dx = fx - half;
            let dy = fy - half;
            let dist_center = (dx * dx + dy * dy).sqrt();

            // Foreground coverage by style. A mute sign wins over the style:
            // the overlay is replaced while Voicemeeter mutes the strip.
            let fg_cov = if mute.is_some() {
                mute_sign_cov(fx, fy, half, s, dist_center)
            } else {
                match style {
                    VisualStyle::Dot => {
                        let dot_r = s * 0.40; // diameter 80%
                        (0.5 - (dist_center - dot_r)).clamp(0.0, 1.0)
                    }
                    VisualStyle::Ring => {
                        // Ring: outer ~80% diameter, thickness ~10% of size.
                        let r_mid = s * 0.35;
                        let half_thick = s * 0.05;
                        (half_thick + 0.5 - (dist_center - r_mid).abs()).clamp(0.0, 1.0)
                    }
                    VisualStyle::Bar => {
                        // Rounded bar SDF.
                        let bqx = (fx - bar_cx).abs() - (bar_hw - bar_r);
                        let bqy = (fy - bar_cy).abs() - (bar_hh - bar_r);
                        let bax = bqx.max(0.0);
                        let bay = bqy.max(0.0);
                        let b_out = (bax * bax + bay * bay).sqrt();
                        let b_in = bqx.max(bqy).min(0.0);
                        let dist_bar = b_out + b_in - bar_r;
                        (0.5 - dist_bar).clamp(0.0, 1.0)
                    }
                    VisualStyle::Mini => {
                        let mini_r = s * 0.27; // diameter ~54%
                        (0.5 - (dist_center - mini_r)).clamp(0.0, 1.0)
                    }
                    VisualStyle::Wave => {
                        // Capsule (vertical segment + radius) per bar; take the
                        // closest bar so adjacent capsules blend smoothly.
                        let mut cov = 0.0f32;
                        let cap_r = bar_w * 0.5;
                        for (i, height) in heights.iter().take(bars).enumerate() {
                            let cx = wave_x0 + i as f32 * wave_step;
                            let seg = (height * 0.5 - cap_r).max(0.0);
                            let ddx = (fx - cx).abs();
                            let ddy = (fy - half).abs() - seg;
                            let ox = ddx.max(0.0);
                            let oy = ddy.max(0.0);
                            let dist = (ox * ox + oy * oy).sqrt() + ddx.max(ddy).min(0.0) - cap_r;
                            cov = cov.max((0.5 - dist).clamp(0.0, 1.0));
                        }
                        cov
                    }
                }
            };

            // Composite: bg first (none for Mini), then fg over it.
            let mut r: f32;
            let mut g: f32;
            let mut b: f32;
            let mut a: f32;
            if style == VisualStyle::Mini {
                r = 0.0;
                g = 0.0;
                b = 0.0;
                a = 0.0;
            } else {
                r = bg.0 as f32;
                g = bg.1 as f32;
                b = bg.2 as f32;
                a = cov_bg * (bg_alpha / 255.0);
            }

            // fg over bg: out_a = fg_a + bg_a*(1-fg_a)
            let (frgb, fggb, fbgb) = mute.unwrap_or(dot);
            let (fr, fgc, fb) = (frgb as f32, fggb as f32, fbgb as f32);
            if fg_cov > 0.0 {
                let out_a = fg_cov + a * (1.0 - fg_cov);
                if out_a > 0.0001 {
                    r = (fr * fg_cov + r * a * (1.0 - fg_cov)) / out_a;
                    g = (fgc * fg_cov + g * a * (1.0 - fg_cov)) / out_a;
                    b = (fb * fg_cov + b * a * (1.0 - fg_cov)) / out_a;
                }
                a = out_a;
            }

            // Unlocked outline: bright ring centered at dist_rr = -1, width ~2px.
            // Drawn even for Mini so drag bounds stay visible.
            if unlocked {
                let ring = (1.0 - (dist_rr + 1.0).abs()).clamp(0.0, 1.0);
                if ring > 0.0 {
                    let oa = ring * 0.85;
                    let out_a = oa + a * (1.0 - oa);
                    if out_a > 0.0001 {
                        r = (225.0 * oa + r * a * (1.0 - oa)) / out_a;
                        g = (225.0 * oa + g * a * (1.0 - oa)) / out_a;
                        b = (225.0 * oa + b * a * (1.0 - oa)) / out_a;
                    }
                    a = out_a;
                }
            }

            // Premultiply
            let ai = (a * 255.0).round().clamp(0.0, 255.0) as u32;
            let ri = (r * a).round().clamp(0.0, 255.0) as u32;
            let gi = (g * a).round().clamp(0.0, 255.0) as u32;
            let bi = (b * a).round().clamp(0.0, 255.0) as u32;
            // DIB 32bpp: 0xAARRGGBB (memory order B,G,R,A on little-endian)
            px[(y as usize) * (size as usize) + (x as usize)] =
                (ai << 24) | (ri << 16) | (gi << 8) | bi;
        }
    }
    true
}

/// Prohibition sign coverage: a ring plus a diagonal slash, sized as a
/// fraction of the overlay. Drawn instead of the style glyph while the
/// Voicemeeter strip is muted.
fn mute_sign_cov(fx: f32, fy: f32, half: f32, s: f32, dist_center: f32) -> f32 {
    let ring_r = s * 0.30;
    let thick = s * 0.055;
    let ring = (thick + 0.5 - (dist_center - ring_r).abs()).clamp(0.0, 1.0);

    // Slash runs from top-left to bottom-right and ends on the ring.
    let k = s * 0.18;
    let (x0, y0) = (half - k, half - k);
    let (bx, by) = (k * 2.0, k * 2.0);
    let (px, py) = (fx - x0, fy - y0);
    let t = ((px * bx + py * by) / (bx * bx + by * by)).clamp(0.0, 1.0);
    let ex = px - bx * t;
    let ey = py - by * t;
    let slash = (thick + 0.5 - (ex * ex + ey * ey).sqrt()).clamp(0.0, 1.0);

    ring.max(slash)
}

/// A reusable 32-bit top-down DIB + memory DC for one window size. Keeping
/// it alive across frames means the animation never allocates a bitmap or a
/// pixel buffer per tick.
pub struct Surface {
    hdc_screen: HDC,
    hdc_mem: HDC,
    hbmp: HBITMAP,
    old_bmp: HGDIOBJ,
    bits: *mut std::ffi::c_void,
    width: u32,
    height: u32,
}

impl Surface {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        unsafe {
            let hdc_screen = GetDC(None);
            if hdc_screen.is_invalid() {
                return None;
            }
            let hdc_mem = CreateCompatibleDC(Some(hdc_screen));
            if hdc_mem.is_invalid() {
                let _ = ReleaseDC(None, hdc_screen);
                return None;
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32), // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [RGBQUAD::default()],
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let Ok(hbmp) =
                CreateDIBSection(Some(hdc_mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            else {
                let _ = DeleteDC(hdc_mem);
                let _ = ReleaseDC(None, hdc_screen);
                return None;
            };
            if bits.is_null() {
                let _ = DeleteObject(hbmp.into());
                let _ = DeleteDC(hdc_mem);
                let _ = ReleaseDC(None, hdc_screen);
                return None;
            }
            let old_bmp = SelectObject(hdc_mem, hbmp.into());
            Some(Self {
                hdc_screen,
                hdc_mem,
                hbmp,
                old_bmp,
                bits,
                width,
                height,
            })
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Copy pixels (BGRA premultiplied, top-down) and push them to a layered
    /// window. Skips the copy when the destination is a different size.
    pub fn upload(&mut self, hwnd: HWND, pixels: &[u32], opacity_pct: u32) {
        let count = (self.width as usize * self.height as usize).min(pixels.len());
        if count == 0 {
            return;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                pixels.as_ptr() as *const u8,
                self.bits as *mut u8,
                count * 4,
            );
            let alpha = ((opacity_pct.clamp(20, 100) * 255) / 100) as u8;
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let size_win = SIZE {
                cx: self.width as i32,
                cy: self.height as i32,
            };
            let pt_src = POINT { x: 0, y: 0 };
            let _ = UpdateLayeredWindow(
                hwnd,
                Some(self.hdc_screen),
                None, // keep current position
                Some(&size_win as *const SIZE),
                Some(self.hdc_mem),
                Some(&pt_src as *const POINT),
                COLORREF(0),
                Some(&blend as *const BLENDFUNCTION),
                ULW_ALPHA,
            );
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.hdc_mem, self.old_bmp);
            let _ = DeleteObject(self.hbmp.into());
            let _ = DeleteDC(self.hdc_mem);
            let _ = ReleaseDC(None, self.hdc_screen);
        }
    }
}

/// Push a non-square premultiplied BGRA bitmap to a layered window. Short
/// lived helper for menus and the tray icon; the overlay keeps a `Surface`.
pub fn update_layered_window_rect(
    hwnd: HWND,
    pixels: &[u32],
    width: u32,
    height: u32,
    opacity_pct: u32,
) {
    if let Some(mut surface) = Surface::new(width, height) {
        surface.upload(hwnd, pixels, opacity_pct);
    }
}

/// Build a small tray icon (16x16 or SM_CXSMICON) with the same rasterizer.
/// Always Dot style with opaque bg for recognizability. Returns None on failure.
pub fn make_tray_icon(dot: (u8, u8, u8), bg: (u8, u8, u8)) -> Option<HICON> {
    unsafe {
        let dim = GetSystemMetrics(SM_CXSMICON);
        let dim = if dim <= 0 { 16 } else { dim.min(32) } as u32;
        let pixels = draw_overlay(dim, bg, dot, false, VisualStyle::Dot, 90, None);

        // Color bitmap (32bpp, top-down DIB -> HBITMAP)
        let hdc_screen = GetDC(None);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: dim as i32,
                biHeight: -(dim as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbReserved: 0,
            }],
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        // Standalone DC not needed for CreateDIBSection; pass None HDC is allowed? Use screen DC.
        let hdc_tmp = CreateCompatibleDC(Some(hdc_screen));
        let hbm_color = CreateDIBSection(Some(hdc_tmp), &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
        let _ = DeleteDC(hdc_tmp);
        let _ = ReleaseDC(None, hdc_screen);
        let Ok(hbm_color) = hbm_color else {
            return None;
        };
        if !bits.is_null() {
            std::ptr::copy_nonoverlapping(
                pixels.as_ptr() as *const u8,
                bits as *mut u8,
                pixels.len() * 4,
            );
        }
        // 1bpp mask (all zero = opaque when combined with alpha color bitmap).
        // CreateBitmap with a null bits pointer leaves the mask uninitialized,
        // which can make the icon render as random garbage; pass zeroed,
        // word-aligned scanlines explicitly.
        let mask_stride = (dim as usize).div_ceil(16) * 2;
        let mask_bits = vec![0u8; mask_stride * dim as usize];
        let hbm_mask = CreateBitmap(
            dim as i32,
            dim as i32,
            1,
            1,
            Some(mask_bits.as_ptr() as *const std::ffi::c_void),
        );
        let icon_info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbm_mask,
            hbmColor: hbm_color,
        };
        let hicon = CreateIconIndirect(&icon_info).ok();
        let _ = DeleteObject(hbm_color.into());
        let _ = DeleteObject(hbm_mask.into());
        hicon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mute_sign_replaces_the_style_glyph() {
        const S: u32 = 48;
        let bg = (0x14, 0x14, 0x14);
        let dot = (0x00, 0xFF, 0x00);
        let mute = (0xE7, 0x4C, 0x3C);

        let normal = draw_overlay(S, bg, dot, false, VisualStyle::Dot, 90, None);
        let mut muted = vec![0u32; (S as usize) * (S as usize)];
        assert!(draw_overlay_into(
            &mut muted,
            S,
            bg,
            dot,
            false,
            VisualStyle::Dot,
            90,
            None,
            Some(mute),
        ));
        let at = |px: &[u32], x: usize, y: usize| px[y * S as usize + x];

        // The slash crosses the centre in mute colour, not the dot colour.
        let centre = at(&muted, 24, 24);
        assert_eq!((centre >> 16) & 0xFF, 0xE7);
        assert_eq!((centre >> 8) & 0xFF, 0x4C);
        assert_eq!(centre & 0xFF, 0x3C);
        assert_eq!((at(&normal, 24, 24) >> 8) & 0xFF, 0xFF);

        // Inside the old dot but clear of ring and slash: only background.
        assert_ne!(at(&normal, 24, 16), at(&muted, 24, 16));
        // The ring is drawn in mute colour too.
        assert_eq!((at(&muted, 38, 24) >> 16) & 0xFF, 0xE7);
    }
}
