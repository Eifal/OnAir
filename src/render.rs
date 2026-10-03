use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Draw overlay pixels (BGRA premultiplied, top-down) of `size`x`size`.
///
/// - rounded square bg (`bg`, ~90% alpha, radius 22% of size)
/// - centered filled circle (`dot`, diameter 80% of size)
/// - when `unlocked`, thin light outline around the rounded square
/// - SDF AA: coverage = clamp(0.5 - dist, 0, 1)
pub fn draw_overlay(
    size: u32,
    bg: (u8, u8, u8),
    dot: (u8, u8, u8),
    unlocked: bool,
) -> Vec<u32> {
    let s = size as f32;
    let half = s / 2.0;
    let radius = s * 0.22;
    let dot_r = s * 0.40; // diameter 80%
    // BG base alpha ~90%
    const BG_ALPHA: f32 = 230.0;

    let mut px = vec![0u32; (size as usize) * (size as usize)];
    let box_half = half - radius; // for rounded-rect SDF

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

            // Circle SDF
            let dx = fx - half;
            let dy = fy - half;
            let dist_c = (dx * dx + dy * dy).sqrt() - dot_r;
            let cov_dot = (0.5 - dist_c).clamp(0.0, 1.0);

            // Composite: bg first, then dot over it (both non-premultiplied blend, then premultiply)
            // bg: rgb=bg, a = cov_bg * BG_ALPHA/255
            let mut r = bg.0 as f32;
            let mut g = bg.1 as f32;
            let mut b = bg.2 as f32;
            let mut a = cov_bg * (BG_ALPHA / 255.0);

            // dot over bg: out_a = dot_a + bg_a*(1-dot_a); out_rgb = (dot*dot_a + bg*bg_a*(1-dot_a))/out_a
            let dot_a = cov_dot; // opaque circle with AA edge
            if dot_a > 0.0 {
                let out_a = dot_a + a * (1.0 - dot_a);
                if out_a > 0.0001 {
                    r = (dot.0 as f32 * dot_a + r * a * (1.0 - dot_a)) / out_a;
                    g = (dot.1 as f32 * dot_a + g * a * (1.0 - dot_a)) / out_a;
                    b = (dot.2 as f32 * dot_a + b * a * (1.0 - dot_a)) / out_a;
                }
                a = out_a;
            }

            // Unlocked outline: bright ring centered at dist_rr = -1, width ~2px
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
/// Returns None on failure (caller keeps old icon).
pub fn make_tray_icon(dot: (u8, u8, u8), bg: (u8, u8, u8)) -> Option<HICON> {
    unsafe {
        let dim = GetSystemMetrics(SM_CXSMICON);
        let dim = if dim <= 0 { 16 } else { dim.min(32) } as u32;
        let pixels = draw_overlay(dim, bg, dot, false);

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
