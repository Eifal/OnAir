//! Windows appearance helpers: app theme preference and a font fallback that
//! works on Windows 10 (Segoe UI Variable ships with Windows 11).

use windows::core::PCWSTR;
use windows::Win32::Foundation::{LPARAM, WIN32_ERROR};
use windows::Win32::Graphics::Gdi::{
    EnumFontFamiliesExW, GetDC, ReleaseDC, DEFAULT_CHARSET, LOGFONTW, TEXTMETRICW,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Read `AppsUseLightTheme` from the current user's personalisation key.
/// Missing values and access errors keep the dark default.
pub fn apps_use_light_theme() -> bool {
    let subkey = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value_name = wide("AppsUseLightTheme");
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    status == WIN32_ERROR(0) && value != 0
}

struct FontSearch<'a> {
    family: &'a str,
    found: bool,
}

unsafe extern "system" fn font_enum_proc(
    logfont: *const LOGFONTW,
    _metric: *const TEXTMETRICW,
    _font_type: u32,
    lparam: LPARAM,
) -> i32 {
    let search = unsafe { &mut *(lparam.0 as *mut FontSearch) };
    let name = unsafe { &(*logfont).lfFaceName };
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    if String::from_utf16_lossy(&name[..end]).eq_ignore_ascii_case(search.family) {
        search.found = true;
        0 // stop enumerating
    } else {
        1 // keep going
    }
}

/// Whether the given font family is installed. GDI is authoritative here:
/// checking for one specific file name misses Windows 11's segmented Segoe UI
/// Variable files, and the renderer matches the family name, not the file.
fn font_family_installed(family: &str) -> bool {
    let mut face = [0u16; 32];
    for (dst, src) in face.iter_mut().zip(family.encode_utf16()) {
        *dst = src;
    }
    let logfont = LOGFONTW {
        lfCharSet: DEFAULT_CHARSET,
        lfFaceName: face,
        ..Default::default()
    };

    let mut search = FontSearch {
        family,
        found: false,
    };
    unsafe {
        let hdc = GetDC(None);
        if hdc.is_invalid() {
            return false;
        }
        EnumFontFamiliesExW(
            hdc,
            &logfont,
            Some(font_enum_proc),
            LPARAM(&mut search as *mut _ as isize),
            0,
        );
        let _ = ReleaseDC(None, hdc);
    }
    search.found
}

/// Segoe UI Variable is available on Windows 11; Windows 10 falls back to the
/// classic Segoe UI so text stays native on both platforms.
pub fn ui_font() -> &'static str {
    // "Segoe UI Variable" is the typographic family name, but the installable
    // families are split into Display/Text/Small. Ask for the UI size that the
    // name table actually exposes.
    if font_family_installed("Segoe UI Variable Text") {
        "Segoe UI Variable Text"
    } else {
        "Segoe UI"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_classic_segoe_ui() {
        assert!(font_family_installed("Segoe UI"));
    }

    #[test]
    fn rejects_missing_family() {
        assert!(!font_family_installed("OnAir Definitively Missing Font"));
    }

    #[test]
    fn selected_ui_font_is_installed() {
        assert!(font_family_installed(ui_font()));
    }
}
