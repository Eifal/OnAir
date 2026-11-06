//! Windows appearance helpers: app theme preference and a font fallback that
//! works on Windows 10 (Segoe UI Variable ships with Windows 11).

use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::Foundation::WIN32_ERROR;
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

/// Segoe UI Variable is available on Windows 11; Windows 10 falls back to the
/// classic Segoe UI so text stays native on both platforms.
pub fn ui_font() -> &'static str {
    let windir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    if windir.join("Fonts").join("SegoeUIVF.ttf").exists() {
        "Segoe UI Variable"
    } else {
        "Segoe UI"
    }
}
