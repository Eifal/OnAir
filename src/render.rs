use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::VisualStyle;

/// Draw overlay pixels (BGRA premultiplied, top-down) of `size`x`size`.
///
/// Styles (same square window, only drawing differs):
/// - Dot: rounded square bg + centered filled circle (diameter 80%)
/// - Ring: rounded square bg + colored ring (outer 80%, thickness ~10%)
/// - Bar: rounded square bg + thin colored strip at the bottom
/// - Mini: no bg, just a small filled dot (diameter ~54%)
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
) -> Vec<u32> {
    let s = size as f32;
    let half = s / 2.0;
    let radius = s * 0.22;
    // Background alpha 0..100% -> 0..255. Foreground (dot/ring/bar)
    // stays fully opaque; global `opacity` still applies on top via
    // SourceConstantAlpha in update_layered_window.
    let bg_alpha: f32 = (bg_opacity.clamp(0, 100) as f32 * 255.0 / 100.0).clamp(0.0, 255.0);

    let mut px = vec![0u32; (size as usize) * (size as usize)];
    let box_half = half - radius; // for rounded-rect SDF

    // Bar geometry (shared for Bar style): thin strip at the bottom.
    let bar_cx = half;
    let bar_cy = s * 0.765;
    let bar_hw = s * 0.35; // half width
    let bar_hh = s * 0.085; // half height
    let bar_r = (s * 0.06).min(bar_hw).min(bar_hh);

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

            // Foreground coverage by style.
            let (fg_cov, use_dot_color) = match style {
                VisualStyle::Dot => {
                    let dot_r = s * 0.40; // diameter 80%
                    ((0.5 - (dist_center - dot_r)).clamp(0.0, 1.0), true)
                }
                VisualStyle::Ring => {
                    // Ring: outer ~80% diameter, thickness ~10% of size.
                    let r_mid = s * 0.35;
                    let half_thick = s * 0.05;
                    (
                        (half_thick + 0.5 - (dist_center - r_mid).abs())
                            .clamp(0.0, 1.0),
                        true,
                    )
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
                    ((0.5 - dist_bar).clamp(0.0, 1.0), true)
                }
                VisualStyle::Mini => {
                    let mini_r = s * 0.27; // diameter ~54%
                    ((0.5 - (dist_center - mini_r)).clamp(0.0, 1.0), true)
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
            let (fr, fg, fb) = if use_dot_color {
                (dot.0 as f32, dot.1 as f32, dot.2 as f32)
            } else {
                (dot.0 as f32, dot.1 as f32, dot.2 as f32)
            };
            if fg_cov > 0.0 {
                let out_a = fg_cov + a * (1.0 - fg_cov);
                if out_a > 0.0001 {
                    r = (fr * fg_cov + r * a * (1.0 - fg_cov)) / out_a;
                    g = (fg * fg_cov + g * a * (1.0 - fg_cov)) / out_a;
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
    px
}

/// Push pixels to a layered window. `opacity_pct` 20..100 maps to SourceConstantAlpha.
pub fn update_layered_window(
    hwnd: HWND,
    pixels: &[u32],
    size: u32,
    opacity_pct: u32,
) {
    unsafe {
        let size_i = size as i32;
        // Source DC + 32-bit top-down DIB
        let hdc_screen = GetDC(None);
        let hdc_mem = CreateCompatibleDC(Some(hdc_screen));

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size_i,
                biHeight: -size_i, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbmp = CreateDIBSection(
            Some(hdc_mem),
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        );
        if hbmp.is_err() {
            if !hdc_mem.is_invalid() {
                let _ = DeleteDC(hdc_mem);
            }
            let _ = ReleaseDC(None, hdc_screen);
            return;
        }
        let hbmp = hbmp.unwrap();
        if !bits.is_null() {
            std::ptr::copy_nonoverlapping(
                pixels.as_ptr() as *const u8,
                bits as *mut u8,
                pixels.len() * 4,
            );
        }
        let h_old = SelectObject(hdc_mem, hbmp.into());

        let alpha = ((opacity_pct.clamp(20, 100) * 255) / 100) as u8;
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: alpha,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let size_win = SIZE { cx: size_i, cy: size_i };
        let pt_src = POINT { x: 0, y: 0 };

        let _ = UpdateLayeredWindow(
            hwnd,
            Some(hdc_screen),
            None, // keep current position
            Some(&size_win as *const SIZE),
            Some(hdc_mem),
            Some(&pt_src as *const POINT),
            COLORREF(0),
            Some(&blend as *const BLENDFUNCTION),
            ULW_ALPHA,
        );

        let _ = SelectObject(hdc_mem, h_old);
        let _ = DeleteObject(hbmp.into());
        let _ = DeleteDC(hdc_mem);
        let _ = ReleaseDC(None, hdc_screen);
    }
}

/// Build a small tray icon (16x16 or SM_CXSMICON) with the same rasterizer.
/// Always Dot style with opaque bg for recognizability. Returns None on failure.
pub fn make_tray_icon(dot: (u8, u8, u8), bg: (u8, u8, u8)) -> Option<HICON> {
    unsafe {
        let dim = GetSystemMetrics(SM_CXSMICON);
        let dim = if dim <= 0 { 16 } else { dim.min(32) } as u32;
        let pixels = draw_overlay(dim, bg, dot, false, VisualStyle::Dot, 90);

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
            bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        // Standalone DC not needed for CreateDIBSection; pass None HDC is allowed? Use screen DC.
        let hdc_tmp = CreateCompatibleDC(Some(hdc_screen));
        let hbm_color = CreateDIBSection(Some(hdc_tmp), &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
        let _ = DeleteDC(hdc_tmp);
        let _ = ReleaseDC(None, hdc_screen);
        let Ok(hbm_color) = hbm_color else { return None };
        if !bits.is_null() {
            std::ptr::copy_nonoverlapping(
                pixels.as_ptr() as *const u8,
                bits as *mut u8,
                pixels.len() * 4,
            );
        }
        // 1bpp mask (all zero = opaque when combined with alpha color bitmap)
        let hbm_mask = CreateBitmap(dim as i32, dim as i32, 1, 1, None);
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
