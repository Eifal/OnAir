//! Native frame styling for the Slint window: dark title bar on Windows 10/11
//! and rounded corners where DWM supports them. The Slint surface itself stays
//! solid; nothing here uses Mica, acrylic or blur.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND,
};

use crate::AppWindow;

/// Apply DWM attributes after the Slint window has a native handle (call
/// after `show`, before `run`). Failures are intentionally ignored: themed
/// title bars are cosmetic and unsupported on older builds.
pub fn apply_frame(ui: &AppWindow, dark: bool) {
    let window_handle = ui.window().window_handle();
    let Ok(handle) = HasWindowHandle::window_handle(&window_handle) else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(win32.hwnd.get() as *mut core::ffi::c_void);

    let dark_flag: i32 = i32::from(dark);
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_flag as *const i32 as *const core::ffi::c_void,
            std::mem::size_of_val(&dark_flag) as u32,
        );
        let corners = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corners as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&corners) as u32,
        );
    }
}
