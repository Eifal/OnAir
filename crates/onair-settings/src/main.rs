#![windows_subsystem = "windows"]

mod ipc;

use onair_config::{update_config_file, AppConfig, ConfigKey};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::*;

slint::include_modules!();

const MUTEX_NAME: &str = "Local\\OnAirSettings";
const WINDOW_TITLE: &str = "OnAir Settings";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Create the process-lifetime single-instance mutex. A second launch
/// restores and foregrounds the existing settings window instead.
fn acquire_single_instance() -> bool {
    let name = wide(MUTEX_NAME);
    let mutex = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) };
    if mutex.is_err() {
        return false;
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        focus_existing();
        return false;
    }
    true
}

fn focus_existing() {
    let title = wide(WINDOW_TITLE);
    unsafe {
        if let Ok(hwnd) = FindWindowW(None, PCWSTR(title.as_ptr())) {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    if !acquire_single_instance() {
        return Ok(());
    }

    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()?;

    let cfg = AppConfig::load();
    let ui = AppWindow::new()?;
    ui.set_size_value(cfg.size as i32);
    let running = ipc::overlay_running();
    ui.set_overlay_running(running);
    ui.set_status_text(format!("Config: {}", AppConfig::config_path().display()).into());

    let weak = ui.as_weak();
    ui.on_set_size(move |size| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let size = (size as u32).clamp(16, 512);
        let path = AppConfig::config_path();
        match update_config_file(&path, &[(ConfigKey::Size, size.to_string())]) {
            Ok(()) => {
                ui.set_size_value(size as i32);
                let running = ipc::notify_reload();
                ui.set_overlay_running(running);
                ui.set_status_text(
                    if running {
                        "Saved - overlay reloaded"
                    } else {
                        "Saved - overlay is not running"
                    }
                    .into(),
                );
            }
            Err(err) => {
                ui.set_status_text(format!("Could not save: {err}").into());
            }
        }
    });

    ui.run()
}
