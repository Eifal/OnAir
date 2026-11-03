//! Custom tray menu: acrylic glass background plus a per-pixel-alpha content
//! layer. The background window owns input/activation; the layered content
//! window draws text, hover pills and borders with proper anti-aliasing.
//!
//! Windows 11/10 acrylic is provided by the undocumented
//! SetWindowCompositionAttribute accent policy. When that is unavailable the
//! background window falls back to a translucent dark panel and the menu still
//! works. Native Win32 menus remain available through `menu_style=native`.

use std::sync::Once;

use windows::core::{PCSTR, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME,
    VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::AppConfig;
use crate::render::update_layered_window_rect;
use crate::tray::{build_model, Item};
use crate::window::to_wide;

const GLASS_CLASS: &str = "OnAirGlassMenu";
const CONTENT_CLASS: &str = "OnAirGlassContent";
const HOVER_TIMER: usize = 1;
const HOVER_MS: u32 = 250;

// Sizes at 96 DPI.
const ITEM_H96: i32 = 30;
const SEP_H96: i32 = 9;
const PAD_X96: i32 = 12;
const PAD_Y96: i32 = 5;
const CHECK_W96: i32 = 24;
const ARROW_W96: i32 = 24;
const RADIUS96: i32 = 8;
const MIN_W96: i32 = 190;
/// Deepest cascade we allow (number of windows, root included):
/// root -> Voicemeeter source -> Primary/Secondary -> Strip/Button.
const MAX_MENU_DEPTH: u32 = 4;

const TEXT: (u8, u8, u8) = (0xF5, 0xF5, 0xF5);
const TEXT_DISABLED: (u8, u8, u8) = (0x8C, 0x8C, 0x96);
const ARROW: (u8, u8, u8) = (0xC8, 0xC8, 0xD0);
const HIGHLIGHT: (u8, u8, u8) = (0x00, 0x67, 0xC0);
const FALLBACK_BG: (u8, u8, u8) = (0x2B, 0x2D, 0x33);
// AABBGGRR: alpha 80%, RGB #2B2D33.
const ACCENT_COLOR: u32 = 0xCC33_2D2B;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Action,
    Check,
    Submenu,
    Disabled,
    Separator,
}

struct Row {
    item: usize,
    rect: RECT,
    kind: RowKind,
}

struct Layout {
    width: i32,
    height: i32,
    dpi: u32,
    pad_x: i32,
    check_w: i32,
    arrow_w: i32,
    radius: i32,
    rows: Vec<Row>,
    font: HFONT,
}

struct Menu {
    hwnd: HWND,
    content: HWND,
    parent: Option<HWND>,
    depth: u32,
    items: Vec<Item>,
    layout: Layout,
    view_height: i32,
    scroll: i32,
    hot: Option<usize>,
    pressed: Option<usize>,
    last_mouse: Option<(i32, i32)>,
    open: Option<usize>,
    child: Option<HWND>,
    accent: bool,
}

impl Drop for Menu {
    fn drop(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), HOVER_TIMER);
            if !self.content.is_invalid() {
                let _ = DestroyWindow(self.content);
            }
            if !self.hwnd.is_invalid() {
                let _ = DestroyWindow(self.hwnd);
            }
            if !self.layout.font.is_invalid() {
                let _ = DeleteObject(self.layout.font.into());
            }
        }
    }
}

struct Session {
    windows: Vec<Menu>,
    root: HWND,
    host: HWND,
    previous: HWND,
    done: bool,
    result: Option<u32>,
    opening: bool,
}

static mut SESSION: Option<Session> = None;

#[repr(C)]
struct AccentPolicy {
    state: i32,
    flags: i32,
    color: u32,
    animation_id: i32,
}

#[repr(C)]
struct WindowCompositionAttributeData {
    attrib: u32,
    data: *mut std::ffi::c_void,
    size: usize,
}

type AccentFn = unsafe extern "system" fn(HWND, *mut WindowCompositionAttributeData) -> i32;
static ACCENT_ONCE: Once = Once::new();
static mut ACCENT_FN: Option<AccentFn> = None;

fn session() -> Option<&'static mut Session> {
    unsafe { SESSION.as_mut() }
}

fn scale(v: i32, dpi: u32) -> i32 {
    ((v as i64 * dpi as i64 + 48) / 96) as i32
}

fn accent_fn() -> Option<AccentFn> {
    ACCENT_ONCE.call_once(|| unsafe {
        let name = to_wide("user32.dll");
        let Ok(user32) = GetModuleHandleW(PCWSTR(name.as_ptr())) else {
            return;
        };
        let proc_name = b"SetWindowCompositionAttribute\0";
        if let Some(f) = GetProcAddress(user32, PCSTR(proc_name.as_ptr())) {
            ACCENT_FN = Some(std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                AccentFn,
            >(f));
        }
    });
    unsafe { ACCENT_FN }
}

unsafe fn try_accent(hwnd: HWND) -> bool {
    let Some(f) = accent_fn() else {
        return false;
    };
    for state in [4i32, 3] {
        let mut policy = AccentPolicy {
            state,
            flags: 2,
            color: ACCENT_COLOR,
            animation_id: 0,
        };
        let mut data = WindowCompositionAttributeData {
            attrib: 19, // WCA_ACCENT_POLICY
            data: &mut policy as *mut _ as *mut std::ffi::c_void,
            size: std::mem::size_of::<AccentPolicy>(),
        };
        if f(hwnd, &mut data) != 0 {
            return true;
        }
    }
    false
}

unsafe fn register_classes() -> bool {
    let Ok(hmodule) = GetModuleHandleW(None) else {
        return false;
    };
    let hinstance = HINSTANCE(hmodule.0);
    let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();

    let glass_name = to_wide(GLASS_CLASS);
    let glass = WNDCLASSW {
        // No CS_DROPSHADOW: its rectangular shadow shows under the rounded
        // acrylic edge (especially on the right and bottom).
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(menu_proc),
        hInstance: hinstance,
        hCursor: cursor,
        lpszClassName: PCWSTR(glass_name.as_ptr()),
        ..Default::default()
    };
    let _ = RegisterClassW(&glass);

    let content_name = to_wide(CONTENT_CLASS);
    let content = WNDCLASSW {
        lpfnWndProc: Some(content_proc),
        hInstance: hinstance,
        hCursor: cursor,
        lpszClassName: PCWSTR(content_name.as_ptr()),
        ..Default::default()
    };
    let _ = RegisterClassW(&content);

    // CreateWindowExW below is the real failure check. RegisterClassW can
    // legitimately fail with ERROR_CLASS_ALREADY_EXISTS.
    true
}

fn make_font(dpi: u32) -> HFONT {
    unsafe {
        let height = -((9 * dpi as i32 + 36) / 72).max(9);
        let face = to_wide("Segoe UI");
        CreateFontW(
            height,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            0,
            PCWSTR(face.as_ptr()),
        )
    }
}

fn item_kind(item: &Item) -> RowKind {
    match item {
        Item::Action { .. } => RowKind::Action,
        Item::Check { .. } => RowKind::Check,
        Item::Submenu { .. } => RowKind::Submenu,
        Item::Disabled { .. } => RowKind::Disabled,
        Item::Separator => RowKind::Separator,
    }
}

fn item_label(item: &Item) -> &str {
    match item {
        Item::Action { label, .. }
        | Item::Check { label, .. }
        | Item::Disabled { label }
        | Item::Submenu { label, .. } => label,
        Item::Separator => "",
    }
}

fn is_enabled(kind: RowKind) -> bool {
    !matches!(kind, RowKind::Disabled | RowKind::Separator)
}

fn measure_labels(items: &[Item], font: HFONT) -> i32 {
    unsafe {
        if font.is_invalid() {
            return 0;
        }
        let hdc = CreateCompatibleDC(None);
        if hdc.is_invalid() {
            return 0;
        }
        let old = SelectObject(hdc, font.into());
        let mut max_w = 0i32;
        for item in items {
            if item_kind(item) == RowKind::Separator {
                continue;
            }
            let mut chars: Vec<u16> = item_label(item).encode_utf16().collect();
            if chars.is_empty() {
                continue;
            }
            let mut sz = SIZE::default();
            if GetTextExtentPoint32W(hdc, &chars, &mut sz).as_bool() {
                max_w = max_w.max(sz.cx);
            }
            chars.clear();
        }
        let _ = SelectObject(hdc, old);
        let _ = DeleteDC(hdc);
        max_w
    }
}

fn build_layout(items: &[Item], dpi: u32, font: HFONT) -> Layout {
    let pad_x = scale(PAD_X96, dpi);
    let pad_y = scale(PAD_Y96, dpi);
    let check_w = scale(CHECK_W96, dpi);
    let arrow_w = scale(ARROW_W96, dpi);
    let radius = scale(RADIUS96, dpi);
    let item_h = scale(ITEM_H96, dpi);
    let sep_h = scale(SEP_H96, dpi).max(1);
    let max_label = measure_labels(items, font);
    let width =
        (pad_x * 2 + check_w + max_label + arrow_w + scale(4, dpi)).max(scale(MIN_W96, dpi));

    let mut rows = Vec::with_capacity(items.len());
    let mut y = pad_y;
    for (i, item) in items.iter().enumerate() {
        let kind = item_kind(item);
        let h = if kind == RowKind::Separator {
            sep_h
        } else {
            item_h
        };
        rows.push(Row {
            item: i,
            rect: RECT {
                left: 0,
                top: y,
                right: width,
                bottom: y + h,
            },
            kind,
        });
        y += h;
    }
    let height = y + pad_y;

    Layout {
        width,
        height,
        dpi,
        pad_x,
        check_w,
        arrow_w,
        radius,
        rows,
        font,
    }
}

fn rounded_cov(px: f32, py: f32, l: f32, t: f32, r: f32, b: f32, rad: f32) -> f32 {
    let cx = (l + r) * 0.5;
    let cy = (t + b) * 0.5;
    let hw = (r - l) * 0.5;
    let hh = (b - t) * 0.5;
    let rr = rad.min(hw).min(hh).max(0.0);
    let qx = (px - cx).abs() - (hw - rr);
    let qy = (py - cy).abs() - (hh - rr);
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    let outside = (ox * ox + oy * oy).sqrt();
    let inside = qx.max(qy).min(0.0);
    let d = outside + inside - rr;
    (0.5 - d).clamp(0.0, 1.0)
}

#[allow(clippy::too_many_arguments)]
fn blend_px(px: &mut [u32], w: usize, h: usize, x: i32, y: i32, r: u32, g: u32, b: u32, a: u32) {
    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 || a == 0 {
        return;
    }
    let idx = y as usize * w + x as usize;
    let dst = px[idx];
    let da = (dst >> 24) & 0xFF;
    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;
    let inv = 255 - a.min(255);
    let nr = (r + (dr * inv + 127) / 255).min(255);
    let ng = (g + (dg * inv + 127) / 255).min(255);
    let nb = (b + (db * inv + 127) / 255).min(255);
    let na = (a + (da * inv + 127) / 255).min(255);
    px[idx] = (na << 24) | (nr << 16) | (ng << 8) | nb;
}

#[allow(clippy::too_many_arguments)]
fn draw_segment(
    px: &mut [u32],
    w: usize,
    h: usize,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    half_thick: f32,
    color: (u8, u8, u8),
) {
    let minx = (x0.min(x1) - half_thick - 1.0).floor() as i32;
    let maxx = (x0.max(x1) + half_thick + 1.0).ceil() as i32;
    let miny = (y0.min(y1) - half_thick - 1.0).floor() as i32;
    let maxy = (y0.max(y1) + half_thick + 1.0).ceil() as i32;
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len2 = (dx * dx + dy * dy).max(0.0001);
    for y in miny..=maxy {
        for x in minx..=maxx {
            let pxc = x as f32 + 0.5;
            let pyc = y as f32 + 0.5;
            let t = (((pxc - x0) * dx + (pyc - y0) * dy) / len2).clamp(0.0, 1.0);
            let ex = pxc - (x0 + t * dx);
            let ey = pyc - (y0 + t * dy);
            let dist = (ex * ex + ey * ey).sqrt();
            let cov = (half_thick + 0.5 - dist).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let a = (cov * 255.0).round() as u32;
            let r = (color.0 as f32 * cov).round() as u32;
            let g = (color.1 as f32 * cov).round() as u32;
            let b = (color.2 as f32 * cov).round() as u32;
            blend_px(px, w, h, x, y, r, g, b, a);
        }
    }
}

/// Render labels white-on-black through GDI, then turn luminance into a
/// coverage mask. This avoids ClearType colour fringes on transparent pixels.
fn text_alpha(layout: &Layout, items: &[Item], scroll: i32, view_h: i32) -> Vec<u8> {
    let w = layout.width.max(1) as usize;
    let h = view_h.max(1) as usize;
    let mut out = vec![0u8; w * h];
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        if mem.is_invalid() {
            let _ = ReleaseDC(None, screen);
            return out;
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
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
        let hbmp = match CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(_) => {
                let _ = DeleteDC(mem);
                let _ = ReleaseDC(None, screen);
                return out;
            }
        };
        if bits.is_null() {
            let _ = DeleteObject(hbmp.into());
            let _ = DeleteDC(mem);
            let _ = ReleaseDC(None, screen);
            return out;
        }
        let old_bmp = SelectObject(mem, hbmp.into());
        let old_font = SelectObject(mem, layout.font.into());
        let black = CreateSolidBrush(COLORREF(0));
        let full = RECT {
            left: 0,
            top: 0,
            right: w as i32,
            bottom: h as i32,
        };
        FillRect(mem, &full, black);
        let _ = DeleteObject(black.into());
        SetBkMode(mem, TRANSPARENT);
        SetTextColor(mem, COLORREF(0x00FF_FFFF));

        let format = DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS;
        for row in &layout.rows {
            if row.kind == RowKind::Separator {
                continue;
            }
            let top = row.rect.top - scroll;
            let bottom = row.rect.bottom - scroll;
            if bottom <= 0 || top >= view_h {
                continue;
            }
            let right = if row.kind == RowKind::Submenu {
                layout.width - layout.pad_x - layout.arrow_w
            } else {
                layout.width - layout.pad_x
            };
            let mut rect = RECT {
                left: layout.pad_x + layout.check_w,
                top,
                right,
                bottom,
            };
            let mut text: Vec<u16> = item_label(&items[row.item]).encode_utf16().collect();
            if !text.is_empty() {
                DrawTextW(mem, &mut text, &mut rect, format);
            }
        }

        // GDI may batch drawing into the DIB; flush before reading it back.
        let _ = GdiFlush();

        let p = bits as *const u8;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let a = (*p.add(i)).max(*p.add(i + 1)).max(*p.add(i + 2));
                out[y * w + x] = a;
            }
        }

        let _ = SelectObject(mem, old_font);
        let _ = SelectObject(mem, old_bmp);
        let _ = DeleteObject(hbmp.into());
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(None, screen);
    }
    out
}

fn render_content(menu: &Menu) -> Vec<u32> {
    let layout = &menu.layout;
    let w = layout.width.max(1) as usize;
    let h = menu.view_height.max(1) as usize;
    let mut px = vec![0u32; w * h];
    let scroll = menu.scroll;
    let dpi = layout.dpi;
    let s = dpi as f32 / 96.0;

    // Text coverage, rendered once per refresh.
    let alpha = text_alpha(layout, &menu.items, scroll, menu.view_height);

    for row in &layout.rows {
        let top = row.rect.top - scroll;
        let bottom = row.rect.bottom - scroll;
        if bottom <= 0 || top >= menu.view_height {
            continue;
        }
        if row.kind == RowKind::Separator {
            let y = (top + bottom) as f32 * 0.5;
            let l = layout.pad_x as f32;
            let r = (layout.width - layout.pad_x) as f32;
            let line = (1.0 * s).max(1.0) * 0.5;
            for yy in (y - line - 1.0).floor() as i32..=(y + line + 1.0).ceil() as i32 {
                for xx in l as i32..=r as i32 {
                    let cov = (line + 0.5 - (yy as f32 + 0.5 - y).abs()).clamp(0.0, 1.0);
                    if cov > 0.0 {
                        let a = (cov * 30.0).round() as u32;
                        blend_px(&mut px, w, h, xx, yy, a, a, a, a);
                    }
                }
            }
            continue;
        }

        let highlighted =
            (menu.hot == Some(row.item) || menu.open == Some(row.item)) && is_enabled(row.kind);

        if highlighted {
            let l = (6.0 * s).round();
            let r = layout.width as f32 - (6.0 * s).round();
            let t = top as f32 + (3.0 * s).round();
            let b = bottom as f32 - (3.0 * s).round();
            let rad = (6.0 * s).round();
            let minx = l.floor() as i32;
            let maxx = r.ceil() as i32;
            let miny = t.floor() as i32;
            let maxy = b.ceil() as i32;
            for yy in miny..=maxy {
                for xx in minx..=maxx {
                    let cov = rounded_cov(xx as f32 + 0.5, yy as f32 + 0.5, l, t, r, b, rad);
                    if cov <= 0.0 {
                        continue;
                    }
                    let a = (cov * 242.0).round() as u32;
                    let rr = (HIGHLIGHT.0 as f32 * cov * 242.0 / 255.0).round() as u32;
                    let gg = (HIGHLIGHT.1 as f32 * cov * 242.0 / 255.0).round() as u32;
                    let bb = (HIGHLIGHT.2 as f32 * cov * 242.0 / 255.0).round() as u32;
                    blend_px(&mut px, w, h, xx, yy, rr, gg, bb, a);
                }
            }
        }

        let cy = (top + bottom) as f32 * 0.5;
        if row.kind == RowKind::Check {
            if let Item::Check { checked: true, .. } = &menu.items[row.item] {
                let cx = (layout.pad_x + layout.check_w / 2) as f32;
                let half = (1.05 * s).max(0.8);
                draw_segment(
                    &mut px,
                    w,
                    h,
                    cx - 4.2 * s,
                    cy + 0.2 * s,
                    cx - 1.2 * s,
                    cy + 3.0 * s,
                    half,
                    TEXT,
                );
                draw_segment(
                    &mut px,
                    w,
                    h,
                    cx - 1.2 * s,
                    cy + 3.0 * s,
                    cx + 4.3 * s,
                    cy - 3.4 * s,
                    half,
                    TEXT,
                );
            }
        }
        if row.kind == RowKind::Submenu {
            let cx = (layout.width - layout.pad_x - layout.arrow_w / 2) as f32;
            let half = (1.0 * s).max(0.75);
            draw_segment(
                &mut px,
                w,
                h,
                cx - 2.0 * s,
                cy - 4.0 * s,
                cx + 2.0 * s,
                cy,
                half,
                ARROW,
            );
            draw_segment(
                &mut px,
                w,
                h,
                cx + 2.0 * s,
                cy,
                cx - 2.0 * s,
                cy + 4.0 * s,
                half,
                ARROW,
            );
        }

        // Composite the label coverage with the row's text colour.
        let color = if row.kind == RowKind::Disabled {
            TEXT_DISABLED
        } else {
            TEXT
        };
        let tx0 = (layout.pad_x + layout.check_w).max(0);
        let tx1 = if row.kind == RowKind::Submenu {
            layout.width - layout.pad_x - layout.arrow_w
        } else {
            layout.width - layout.pad_x
        }
        .min(layout.width);
        let y0 = top.max(0);
        let y1 = bottom.min(menu.view_height);
        for yy in y0..y1 {
            for xx in tx0..tx1 {
                if xx < 0 || xx >= layout.width || yy < 0 || yy >= menu.view_height {
                    continue;
                }
                let a = alpha[yy as usize * w + xx as usize] as u32;
                if a == 0 {
                    continue;
                }
                let rr = (color.0 as u32 * a + 127) / 255;
                let gg = (color.1 as u32 * a + 127) / 255;
                let bb = (color.2 as u32 * a + 127) / 255;
                blend_px(&mut px, w, h, xx, yy, rr, gg, bb, a);
            }
        }
    }

    // Thin scroll indicator in the right padding when the menu is taller
    // than the work area. It stays clear of the labels and submenu arrows.
    let max_scroll = (layout.height - menu.view_height).max(0);
    if max_scroll > 0 {
        let track_w = (2.0 * s).round().max(1.0);
        let x0 = (layout.width as f32 - layout.pad_x as f32 * 0.5 - track_w * 0.5).round();
        let pad_y = scale(PAD_Y96, dpi);
        let y0 = pad_y as f32;
        let y1 = (menu.view_height - pad_y) as f32;
        let track_h = y1 - y0;
        if track_h > 0.0 {
            let paint_bar = |px: &mut [u32], top: f32, bottom: f32, alpha: u32| {
                let rad = track_w * 0.5;
                for yy in top.floor() as i32..=bottom.ceil() as i32 {
                    for xx in x0.floor() as i32..=(x0 + track_w).ceil() as i32 {
                        let cov = rounded_cov(
                            xx as f32 + 0.5,
                            yy as f32 + 0.5,
                            x0,
                            top,
                            x0 + track_w,
                            bottom,
                            rad,
                        );
                        if cov > 0.0 {
                            let a = (cov * alpha as f32).round() as u32;
                            blend_px(px, w, h, xx, yy, a, a, a, a);
                        }
                    }
                }
            };
            paint_bar(&mut px, y0, y1, 18);
            let min_thumb = ((24.0 * s) as i32).max(1) as f32;
            let thumb_h = (track_h * menu.view_height as f32 / layout.height as f32)
                .max(min_thumb)
                .min(track_h);
            let progress = menu.scroll as f32 / max_scroll as f32;
            let thumb_top = y0 + (track_h - thumb_h) * progress;
            paint_bar(&mut px, thumb_top, thumb_top + thumb_h, 90);
        }
    }

    // 1px anti-aliased border. This hides the hard region edge at the corners.
    let rad = layout.radius as f32;
    let bw = layout.width as f32 - 0.5;
    let bh = menu.view_height as f32 - 0.5;
    for y in 0..menu.view_height {
        for x in 0..layout.width {
            let d = {
                let cx = bw * 0.5;
                let cy = bh * 0.5;
                let hw = bw * 0.5;
                let hh = bh * 0.5;
                let rr = rad.min(hw).min(hh).max(0.0);
                let qx = (x as f32 + 0.5 - cx).abs() - (hw - rr);
                let qy = (y as f32 + 0.5 - cy).abs() - (hh - rr);
                let ox = qx.max(0.0);
                let oy = qy.max(0.0);
                (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - rr
            };
            let cov = (1.0 - (d + 0.5).abs()).clamp(0.0, 1.0);
            if cov > 0.0 {
                let a = (cov * 38.0).round() as u32;
                blend_px(&mut px, w, h, x, y, a, a, a, a);
            }
        }
    }

    px
}

unsafe fn refresh_content(menu: &Menu) {
    if menu.content.is_invalid() {
        return;
    }
    let px = render_content(menu);
    update_layered_window_rect(
        menu.content,
        &px,
        menu.layout.width.max(1) as u32,
        menu.view_height.max(1) as u32,
        100,
    );
}

unsafe fn apply_region(hwnd: HWND, w: i32, h: i32, radius: i32) {
    let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, radius * 2, radius * 2);
    if !rgn.is_invalid() && SetWindowRgn(hwnd, Some(rgn), true) == 0 {
        // On success the system owns the region; on failure the caller keeps it.
        let _ = DeleteObject(rgn.into());
    }
}

/// Ask DWM for smooth rounded corners and no system border (Windows 11).
/// Returns false on older systems, where the caller falls back to a hard
/// `SetWindowRgn` clip.
unsafe fn prefer_round_corners(hwnd: HWND) -> bool {
    let preference = DWMWCP_ROUND;
    let rounded = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &preference as *const _ as *const std::ffi::c_void,
        std::mem::size_of_val(&preference) as u32,
    )
    .is_ok();

    if rounded {
        // Windows 11 otherwise paints its own light border on the right and
        // bottom edges, which looks like a one-sided shadow. Our content
        // layer already draws a consistent border.
        let no_border = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &no_border as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&no_border) as u32,
        );
    }
    rounded
}

fn work_area_from_point(pt: POINT) -> RECT {
    unsafe {
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            }
        }
    }
}

fn work_area_from_window(hwnd: HWND) -> RECT {
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            }
        }
    }
}

fn root_position(cursor: POINT, w: i32, h: i32) -> (i32, i32) {
    let work = work_area_from_point(cursor);
    let mut x = cursor.x;
    let mut y = cursor.y - h; // TPM_BOTTOMALIGN
    if y < work.top {
        y = cursor.y; // no room above; open downwards
    }
    if x + w > work.right {
        x = work.right - w;
    }
    if x < work.left {
        x = work.left;
    }
    if y + h > work.bottom {
        y = work.bottom - h;
    }
    if y < work.top {
        y = work.top;
    }
    // A very tall (high-DPI) menu can be clamped over the cursor. Move it
    // sideways so the cursor starts outside; otherwise Windows' synthetic
    // mouse move could open a submenu under the pointer immediately.
    if y < cursor.y && cursor.y < y + h {
        let gap = 12;
        if cursor.x + gap + w <= work.right {
            x = cursor.x + gap;
        } else if cursor.x - gap - w >= work.left {
            x = cursor.x - gap - w;
        }
    }
    (x, y)
}

fn submenu_position(parent: HWND, item_top: i32, w: i32, h: i32) -> (i32, i32) {
    unsafe {
        let mut pr = RECT::default();
        let _ = GetWindowRect(parent, &mut pr);
        let work = work_area_from_window(parent);
        let mut x = pr.right - scale(2, 96);
        if x + w > work.right {
            x = pr.left - w + scale(2, 96);
        }
        if x + w > work.right {
            x = work.right - w;
        }
        if x < work.left {
            x = work.left;
        }
        let mut y = item_top;
        if y + h > work.bottom {
            y = work.bottom - h;
        }
        if y < work.top {
            y = work.top;
        }
        (x, y)
    }
}

unsafe fn create_menu(
    s: &mut Session,
    items: Vec<Item>,
    parent: Option<HWND>,
    depth: u32,
    anchor: POINT,
    max_h: i32,
) -> Option<HWND> {
    let Ok(hmodule) = GetModuleHandleW(None) else {
        return None;
    };
    let hinstance = HINSTANCE(hmodule.0);
    let class = to_wide(GLASS_CLASS);
    let title = to_wide("OnAir menu");
    let owner = parent.or_else(|| {
        if s.host.is_invalid() {
            None
        } else {
            Some(s.host)
        }
    });
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        PCWSTR(class.as_ptr()),
        PCWSTR(title.as_ptr()),
        WS_POPUP,
        anchor.x,
        anchor.y,
        1,
        1,
        owner,
        None,
        Some(hinstance),
        None,
    )
    .ok()?;

    let dpi = {
        // Submenus inherit the DPI of their parent menu; the root menu uses
        // the monitor under the cursor, which is where it was just created.
        let d = GetDpiForWindow(parent.unwrap_or(hwnd));
        if d == 0 {
            96
        } else {
            d
        }
    };
    let font = make_font(dpi);
    if font.is_invalid() {
        let _ = DestroyWindow(hwnd);
        return None;
    }
    let layout = build_layout(&items, dpi, font);
    let view_h = layout.height.min(max_h.max(scale(40, dpi)));

    let (x, y) = if let Some(parent) = parent {
        submenu_position(parent, anchor.y, layout.width, view_h)
    } else {
        root_position(anchor, layout.width, view_h)
    };
    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        x,
        y,
        layout.width,
        view_h,
        SWP_NOACTIVATE,
    );
    // Windows 11 clips the acrylic with a smooth DWM curve. Only use the
    // hard-edged region on systems where that attribute is unavailable.
    let rounded = prefer_round_corners(hwnd);
    if !rounded {
        apply_region(hwnd, layout.width, view_h, layout.radius);
    }

    let content_class = to_wide(CONTENT_CLASS);
    let content_name = to_wide("");
    let content = match CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_TOPMOST,
        PCWSTR(content_class.as_ptr()),
        PCWSTR(content_name.as_ptr()),
        WS_POPUP,
        x,
        y,
        layout.width,
        view_h,
        Some(hwnd),
        None,
        Some(hinstance),
        None,
    ) {
        Ok(h) => h,
        Err(_) => {
            let _ = DestroyWindow(hwnd);
            let _ = DeleteObject(font.into());
            return None;
        }
    };
    // The content layer is per-pixel-alpha, so it must not get a hard window
    // region; that would clip the anti-aliased border into stair steps.

    let menu = Menu {
        hwnd,
        content,
        parent,
        depth,
        items,
        layout,
        view_height: view_h,
        scroll: 0,
        hot: None,
        pressed: None,
        last_mouse: None,
        open: None,
        child: None,
        accent: false,
    };
    s.windows.push(menu);
    let idx = s.windows.len() - 1;

    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    let accent = try_accent(hwnd);
    s.windows[idx].accent = accent;
    if !accent {
        // No acrylic: a translucent dark panel, still with rounded corners.
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 235, LWA_ALPHA);
        // DWM corner rounding is unreliable on a layered fallback window.
        let (fallback_w, fallback_h, fallback_radius) = {
            let menu = &s.windows[idx];
            (menu.layout.width, menu.view_height, menu.layout.radius)
        };
        apply_region(hwnd, fallback_w, fallback_h, fallback_radius);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
    refresh_content(&s.windows[idx]);
    let _ = ShowWindow(content, SW_SHOWNOACTIVATE);
    let _ = SetWindowPos(
        content,
        Some(HWND_TOPMOST),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE,
    );
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(Some(hwnd));
    Some(hwnd)
}

fn menu_index(s: &Session, hwnd: HWND) -> Option<usize> {
    s.windows.iter().position(|m| m.hwnd == hwnd)
}

fn is_session_window(s: &Session, hwnd: HWND) -> bool {
    if hwnd.is_invalid() {
        return false;
    }
    if !s.host.is_invalid() && hwnd == s.host {
        return true;
    }
    s.windows
        .iter()
        .any(|m| m.hwnd == hwnd || m.content == hwnd)
}

fn close_session(s: &mut Session, result: Option<u32>) {
    if s.done {
        return;
    }
    s.done = true;
    s.result = result;
    for m in &s.windows {
        unsafe {
            let _ = KillTimer(Some(m.hwnd), HOVER_TIMER);
            let _ = ShowWindow(m.content, SW_HIDE);
            let _ = ShowWindow(m.hwnd, SW_HIDE);
        }
    }
}

fn cleanup_session() -> Option<u32> {
    let mut s = unsafe { SESSION.take() }?;
    for m in &s.windows {
        unsafe {
            let _ = KillTimer(Some(m.hwnd), HOVER_TIMER);
            let _ = ShowWindow(m.content, SW_HIDE);
            let _ = ShowWindow(m.hwnd, SW_HIDE);
        }
    }
    let result = s.result;
    let previous = s.previous;
    let host = s.host;
    s.windows.clear();
    unsafe {
        if !host.is_invalid() {
            let _ = DestroyWindow(host);
        }
        if !previous.is_invalid() && previous != host {
            let _ = SetForegroundWindow(previous);
        }
    }
    result
}

fn collect_tree(s: &Session, hwnd: HWND, out: &mut Vec<HWND>) {
    out.push(hwnd);
    if let Some(idx) = menu_index(s, hwnd) {
        if let Some(child) = s.windows[idx].child {
            collect_tree(s, child, out);
        }
    }
}

fn close_submenu(s: &mut Session, parent_idx: usize, keep_hot: bool) {
    let child = s.windows[parent_idx].child.take();
    let open = s.windows[parent_idx].open.take();
    if keep_hot {
        s.windows[parent_idx].hot = open;
    }
    if let Some(child) = child {
        let mut tree = Vec::new();
        collect_tree(s, child, &mut tree);
        for h in tree.iter().rev() {
            if let Some(idx) = menu_index(s, *h) {
                unsafe {
                    let _ = KillTimer(Some(s.windows[idx].hwnd), HOVER_TIMER);
                }
                s.windows.remove(idx);
            }
        }
    }
    unsafe {
        let parent_hwnd = s.windows[parent_idx].hwnd;
        refresh_content(&s.windows[parent_idx]);
        let _ = SetForegroundWindow(parent_hwnd);
        let _ = SetFocus(Some(parent_hwnd));
    }
}

fn depth_of(s: &Session, hwnd: HWND) -> u32 {
    let mut d = 0;
    let mut cur = Some(hwnd);
    while let Some(h) = cur {
        d += 1;
        cur = menu_index(s, h).and_then(|i| s.windows[i].parent);
    }
    d
}

fn first_enabled(rows: &[Row]) -> Option<usize> {
    rows.iter().find(|r| is_enabled(r.kind)).map(|r| r.item)
}

fn row_for_item(menu: &Menu, item: usize) -> Option<&Row> {
    menu.layout.rows.iter().find(|r| r.item == item)
}

fn scroll_to_row(menu: &mut Menu, item: usize) {
    if let Some(row) = row_for_item(menu, item) {
        let top = row.rect.top;
        let bottom = row.rect.bottom;
        if top < menu.scroll {
            menu.scroll = top;
        } else if bottom > menu.scroll + menu.view_height {
            menu.scroll = bottom - menu.view_height;
        }
        let max = (menu.layout.height - menu.view_height).max(0);
        menu.scroll = menu.scroll.clamp(0, max);
    }
}

unsafe fn open_submenu(s: &mut Session, parent_idx: usize, item_idx: usize, select_first: bool) {
    if s.windows[parent_idx].open == Some(item_idx) && s.windows[parent_idx].child.is_some() {
        return;
    }
    if depth_of(s, s.windows[parent_idx].hwnd) >= MAX_MENU_DEPTH {
        return;
    }
    let parent_hwnd = s.windows[parent_idx].hwnd;
    let depth = s.windows[parent_idx].depth + 1;
    let (items, anchor) = {
        let menu = &s.windows[parent_idx];
        let Some(row) = row_for_item(menu, item_idx) else {
            return;
        };
        let Item::Submenu { items, .. } = &menu.items[item_idx] else {
            return;
        };
        // `row.rect` is in layout space, so subtract the current scroll
        // offset before converting to screen coordinates. The x coordinate
        // only influences monitor/DPI detection for the new window.
        let mut pt = POINT {
            x: menu.layout.width,
            y: row.rect.top - menu.scroll,
        };
        let _ = ClientToScreen(menu.hwnd, &mut pt);
        (items.clone(), pt)
    };

    if s.windows[parent_idx].child.is_some() {
        close_submenu(s, parent_idx, true);
    }
    let work = work_area_from_window(parent_hwnd);
    let max_h = (work.bottom - work.top - scale(8, 96)).max(scale(120, 96));
    let parent_idx = menu_index(s, parent_hwnd).unwrap_or(parent_idx);
    s.opening = true;
    let created = create_menu(s, items, Some(parent_hwnd), depth, anchor, max_h);
    s.opening = false;
    if let Some(child) = created {
        s.windows[parent_idx].child = Some(child);
        s.windows[parent_idx].open = Some(item_idx);
        refresh_content(&s.windows[parent_idx]);
        if select_first {
            if let Some(ci) = menu_index(s, child) {
                let first = first_enabled(&s.windows[ci].layout.rows);
                s.windows[ci].hot = first;
                if let Some(first) = first {
                    scroll_to_row(&mut s.windows[ci], first);
                }
                refresh_content(&s.windows[ci]);
            }
        }
    }
}

fn move_selection(s: &mut Session, idx: usize, delta: i32, absolute: Option<bool>) {
    let menu = &mut s.windows[idx];
    let enabled: Vec<usize> = menu
        .layout
        .rows
        .iter()
        .filter(|r| is_enabled(r.kind))
        .map(|r| r.item)
        .collect();
    if enabled.is_empty() {
        return;
    }
    let pos = menu.hot.and_then(|h| enabled.iter().position(|i| *i == h));
    let next_pos = if let Some(first) = absolute {
        if first {
            0
        } else {
            enabled.len() - 1
        }
    } else {
        match (pos, delta) {
            (Some(p), d) if d > 0 => (p + 1) % enabled.len(),
            (Some(p), _) => (p + enabled.len() - 1) % enabled.len(),
            (None, d) if d > 0 => 0,
            (None, _) => enabled.len() - 1,
        }
    };
    let item = enabled[next_pos];
    if menu.child.is_some() {
        close_submenu(s, idx, false);
    }
    s.windows[idx].hot = Some(item);
    scroll_to_row(&mut s.windows[idx], item);
    unsafe {
        refresh_content(&s.windows[idx]);
    }
}

unsafe fn activate_item(s: &mut Session, idx: usize, item: usize, select_first: bool) {
    match &s.windows[idx].items[item] {
        Item::Action { id, .. } | Item::Check { id, .. } => {
            let id = *id;
            close_session(s, Some(id));
        }
        Item::Submenu { .. } => open_submenu(s, idx, item, select_first),
        Item::Disabled { .. } | Item::Separator => {}
    }
}

/// The scroll indicator is painted over the right padding; clicks there
/// should scroll, not activate the row behind it.
fn scroll_gutter(menu: &Menu, x: i32, y: i32) -> bool {
    if menu.layout.height <= menu.view_height {
        return false;
    }
    let s = menu.layout.dpi as f32 / 96.0;
    let track_w = (2.0 * s).round().max(1.0);
    let x0 = (menu.layout.width as f32 - menu.layout.pad_x as f32 * 0.5 - track_w * 0.5).round();
    let pad_y = scale(PAD_Y96, menu.layout.dpi);
    (x as f32) >= x0 && (x as f32) < x0 + track_w && y >= pad_y && y < menu.view_height - pad_y
}

fn hit_test(menu: &Menu, x: i32, y: i32) -> Option<usize> {
    if x < 0
        || x >= menu.layout.width
        || y < 0
        || y >= menu.view_height
        || scroll_gutter(menu, x, y)
    {
        return None;
    }
    let cy = y + menu.scroll;
    menu.layout
        .rows
        .iter()
        .find(|r| cy >= r.rect.top && cy < r.rect.bottom && is_enabled(r.kind))
        .map(|r| r.item)
}

unsafe fn start_hover_timer(menu: &Menu) {
    if let Some(item) = menu.hot {
        if matches!(menu.items[item], Item::Submenu { .. }) && menu.open != Some(item) {
            let _ = SetTimer(Some(menu.hwnd), HOVER_TIMER, HOVER_MS, None);
            return;
        }
    }
    let _ = KillTimer(Some(menu.hwnd), HOVER_TIMER);
}

unsafe fn ensure_mouse_tracking(hwnd: HWND) {
    let mut tme = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    let _ = TrackMouseEvent(&mut tme);
}

unsafe fn handle_glass_message(
    s: &mut Session,
    idx: usize,
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !s.windows[idx].accent {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let brush = CreateSolidBrush(COLORREF(
                    (FALLBACK_BG.0 as u32)
                        | ((FALLBACK_BG.1 as u32) << 8)
                        | ((FALLBACK_BG.2 as u32) << 16),
                ));
                FillRect(hdc, &rc, brush);
                let _ = DeleteObject(brush.into());
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lp.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            // Windows can deliver a duplicate move when another menu window
            // appears under a stationary cursor. Ignoring it preserves the
            // keyboard selection and keeps a just-opened submenu open.
            if s.windows[idx].last_mouse == Some((x, y)) {
                return LRESULT(0);
            }
            s.windows[idx].last_mouse = Some((x, y));
            ensure_mouse_tracking(hwnd);
            let new_hot = hit_test(&s.windows[idx], x, y);
            let old_hot = s.windows[idx].hot;
            let open = s.windows[idx].open;
            let has_child = s.windows[idx].child.is_some();
            if has_child && new_hot.is_some() && new_hot != open {
                close_submenu(s, idx, false);
            }
            if new_hot != old_hot {
                s.windows[idx].hot = new_hot;
                refresh_content(&s.windows[idx]);
                // A held mouse button must not auto-open a submenu.
                if s.windows[idx].pressed.is_none() {
                    start_hover_timer(&s.windows[idx]);
                } else {
                    let _ = KillTimer(Some(hwnd), HOVER_TIMER);
                }
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            s.windows[idx].pressed = None;
            s.windows[idx].last_mouse = None;
            let has_child = s.windows[idx].child.is_some();
            let old_hot = s.windows[idx].hot;
            if !has_child {
                s.windows[idx].hot = None;
                if old_hot.is_some() {
                    refresh_content(&s.windows[idx]);
                }
            }
            let _ = KillTimer(Some(hwnd), HOVER_TIMER);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lp.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            if s.windows[idx].child.is_some() {
                close_submenu(s, idx, false);
            }
            let pressed = hit_test(&s.windows[idx], x, y);
            s.windows[idx].pressed = pressed;
            if pressed != s.windows[idx].hot {
                s.windows[idx].hot = pressed;
                refresh_content(&s.windows[idx]);
            }
            // Do not let the hover timer open a submenu while the button is held.
            let _ = KillTimer(Some(hwnd), HOVER_TIMER);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let x = (lp.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            let pressed = s.windows[idx].pressed.take();
            let over = hit_test(&s.windows[idx], x, y);
            if let Some(item) = pressed {
                if over == Some(item) {
                    activate_item(s, idx, item, false);
                    return LRESULT(0);
                }
            }
            if over != s.windows[idx].hot {
                s.windows[idx].hot = over;
                refresh_content(&s.windows[idx]);
                start_hover_timer(&s.windows[idx]);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            if delta != 0 {
                let step = scale(ITEM_H96, s.windows[idx].layout.dpi) * 2;
                let max = (s.windows[idx].layout.height - s.windows[idx].view_height).max(0);
                let scroll = (s.windows[idx].scroll - delta / 120 * step).clamp(0, max);
                if scroll != s.windows[idx].scroll {
                    // The submenu is anchored to a row, so it cannot stay open
                    // while the parent list scrolls away from under it.
                    if s.windows[idx].child.is_some() {
                        close_submenu(s, idx, false);
                    }
                    s.windows[idx].pressed = None;
                    s.windows[idx].scroll = scroll;
                    // Re-hit-test the cursor: scrolling moves the rows under it.
                    let mut pt = POINT::default();
                    let hot = if GetCursorPos(&mut pt).is_ok()
                        && ScreenToClient(hwnd, &mut pt).as_bool()
                    {
                        hit_test(&s.windows[idx], pt.x, pt.y)
                    } else {
                        None
                    };
                    s.windows[idx].hot = hot;
                    refresh_content(&s.windows[idx]);
                    start_hover_timer(&s.windows[idx]);
                }
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wp.0 == HOVER_TIMER {
                let _ = KillTimer(Some(hwnd), HOVER_TIMER);
                if let Some(item) = s.windows[idx].hot {
                    if matches!(s.windows[idx].items[item], Item::Submenu { .. })
                        && s.windows[idx].open != Some(item)
                    {
                        open_submenu(s, idx, item, false);
                    }
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = wp.0 as u16;
            if vk == VK_ESCAPE.0 || vk == VK_LEFT.0 {
                if s.windows[idx].child.is_some() {
                    close_submenu(s, idx, true);
                } else if let Some(parent) = s.windows[idx].parent {
                    if let Some(pi) = menu_index(s, parent) {
                        close_submenu(s, pi, true);
                    }
                } else {
                    close_session(s, None);
                }
                return LRESULT(0);
            }
            if vk == VK_UP.0 {
                move_selection(s, idx, -1, None);
                return LRESULT(0);
            }
            if vk == VK_DOWN.0 {
                move_selection(s, idx, 1, None);
                return LRESULT(0);
            }
            if vk == VK_HOME.0 {
                move_selection(s, idx, 1, Some(true));
                return LRESULT(0);
            }
            if vk == VK_END.0 {
                move_selection(s, idx, -1, Some(false));
                return LRESULT(0);
            }
            if vk == VK_RIGHT.0 {
                if let Some(item) = s.windows[idx].hot {
                    if matches!(s.windows[idx].items[item], Item::Submenu { .. }) {
                        open_submenu(s, idx, item, true);
                    }
                }
                return LRESULT(0);
            }
            if vk == VK_RETURN.0 || vk == VK_SPACE.0 {
                if let Some(item) = s.windows[idx].hot {
                    activate_item(s, idx, item, true);
                } else if let Some(item) = first_enabled(&s.windows[idx].layout.rows) {
                    activate_item(s, idx, item, true);
                }
                return LRESULT(0);
            }
            // Jump to the next label starting with a letter/digit.
            let ch = if (0x41..=0x5A).contains(&vk) || (0x30..=0x39).contains(&vk) {
                char::from_u32(vk as u32)
            } else {
                None
            };
            if let Some(ch) = ch {
                let ch = ch.to_ascii_uppercase();
                let rows = &s.windows[idx].layout.rows;
                let start = s.windows[idx]
                    .hot
                    .and_then(|h| rows.iter().position(|r| r.item == h));
                let n = rows.len();
                for off in 0..n {
                    // Without a current selection, start at the first row.
                    let pos = match start {
                        Some(current) => (current + off + 1) % n,
                        None => off,
                    };
                    let r = &rows[pos];
                    if !is_enabled(r.kind) {
                        continue;
                    }
                    if item_label(&s.windows[idx].items[r.item])
                        .chars()
                        .next()
                        .map(|c| c.to_ascii_uppercase() == ch)
                        .unwrap_or(false)
                    {
                        let item = r.item;
                        s.windows[idx].hot = Some(item);
                        scroll_to_row(&mut s.windows[idx], item);
                        refresh_content(&s.windows[idx]);
                        break;
                    }
                }
                return LRESULT(0);
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let state = (wp.0 & 0xFFFF) as u32;
            if state == WA_INACTIVE && !s.opening && !s.done {
                let active = GetForegroundWindow();
                if !is_session_window(s, active) {
                    close_session(s, None);
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            close_session(s, None);
            LRESULT(0)
        }
        WM_DESTROY => {
            if s.root == hwnd {
                close_session(s, None);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn menu_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(s) = session() else {
        return DefWindowProcW(hwnd, msg, wp, lp);
    };
    let Some(idx) = menu_index(s, hwnd) else {
        return DefWindowProcW(hwnd, msg, wp, lp);
    };
    handle_glass_message(s, idx, hwnd, msg, wp, lp)
}

unsafe extern "system" fn content_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Show the custom menu. Falls back to the native Win32 menu if the windows
/// cannot be created. Returns the selected command ID, or None when dismissed.
pub fn show(_hwnd: HWND, cfg: &AppConfig) -> Option<u32> {
    unsafe {
        if !register_classes() {
            return crate::tray::show_menu(_hwnd, cfg);
        }
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor).is_err() {
            return crate::tray::show_menu(_hwnd, cfg);
        }
        let previous = GetForegroundWindow();

        // Hidden helper keeps normal activation semantics for the first click.
        let host_class = to_wide("STATIC");
        let host_title = to_wide("");
        let host = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            PCWSTR(host_class.as_ptr()),
            PCWSTR(host_title.as_ptr()),
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
        .ok()
        .unwrap_or_default();
        if !host.is_invalid() {
            let _ = ShowWindow(host, SW_SHOWNOACTIVATE);
            let _ = SetForegroundWindow(host);
        }

        SESSION = Some(Session {
            windows: Vec::new(),
            root: HWND::default(),
            host,
            previous,
            done: false,
            result: None,
            opening: false,
        });

        let items = build_model(cfg);
        let work = work_area_from_point(cursor);
        let max_h = (work.bottom - work.top - scale(8, 96)).max(scale(120, 96));
        let root = {
            let s = session().unwrap();
            create_menu(s, items, None, 0, cursor, max_h)
        };
        let Some(root) = root else {
            let _ = cleanup_session();
            return crate::tray::show_menu(_hwnd, cfg);
        };
        if let Some(s) = session() {
            s.root = root;
        }
        let _ = SetForegroundWindow(root);
        let _ = SetFocus(Some(root));

        let mut msg = MSG::default();
        loop {
            let done = session().map(|s| s.done).unwrap_or(true);
            if done {
                break;
            }
            let ret = GetMessageW(&mut msg, None, 0, 0);
            if ret.0 == 0 {
                // Preserve WM_QUIT for the main application loop.
                PostQuitMessage(msg.wParam.0 as i32);
                break;
            }
            if ret.0 == -1 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        cleanup_session()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(item: usize, top: i32, bottom: i32, kind: RowKind) -> Row {
        Row {
            item,
            rect: RECT {
                left: 0,
                top,
                right: 200,
                bottom,
            },
            kind,
        }
    }

    fn menu(height: i32, view_height: i32, rows: Vec<Row>) -> Menu {
        Menu {
            hwnd: HWND::default(),
            content: HWND::default(),
            parent: None,
            depth: 0,
            items: Vec::new(),
            layout: Layout {
                width: 200,
                height,
                dpi: 96,
                pad_x: scale(PAD_X96, 96),
                check_w: scale(CHECK_W96, 96),
                arrow_w: scale(ARROW_W96, 96),
                radius: scale(RADIUS96, 96),
                rows,
                font: HFONT::default(),
            },
            view_height,
            scroll: 0,
            hot: None,
            pressed: None,
            last_mouse: None,
            open: None,
            child: None,
            accent: false,
        }
    }

    #[test]
    fn hit_test_uses_scroll_offset_and_skips_disabled_rows() {
        let mut m = menu(
            120,
            60,
            vec![
                row(0, 0, 30, RowKind::Action),
                row(1, 30, 60, RowKind::Submenu),
                row(2, 60, 90, RowKind::Disabled),
            ],
        );
        assert_eq!(hit_test(&m, 10, 10), Some(0));
        assert_eq!(hit_test(&m, 10, 45), Some(1));
        assert_eq!(hit_test(&m, 10, 75), None);
        m.scroll = 20;
        assert_eq!(hit_test(&m, 10, 10), Some(1));
        assert_eq!(hit_test(&m, 10, 50), None);
    }

    #[test]
    fn scroll_gutter_blocks_clicks_on_indicator() {
        let m = menu(
            120,
            60,
            vec![
                row(0, 0, 30, RowKind::Action),
                row(1, 30, 60, RowKind::Submenu),
            ],
        );
        // width 200, pad_x 12, track 2 -> indicator occupies x 193..195.
        assert!(scroll_gutter(&m, 193, 30));
        assert!(scroll_gutter(&m, 194, 30));
        assert!(!scroll_gutter(&m, 192, 30));
        assert!(!scroll_gutter(&m, 100, 30));
        assert!(!scroll_gutter(&m, 193, 4));
        assert!(!scroll_gutter(&m, 193, 56));
        assert_eq!(hit_test(&m, 193, 30), None);
        assert_eq!(hit_test(&m, 100, 30), Some(1));
    }

    #[test]
    fn scroll_gutter_absent_when_menu_fits() {
        let m = menu(60, 60, vec![row(0, 0, 30, RowKind::Action)]);
        assert!(!scroll_gutter(&m, 193, 30));
    }
}
