use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const CLASS_NAME: &str = "OnAirOverlay";
pub const MUTEX_NAME: &str = "Local\\OnAirOverlay";
pub const WM_TRAY: u32 = 1024 + 1; // WM_APP + 1
pub const WM_KEY_CAPTURED: u32 = 1024 + 3; // WM_APP + 3, capture result
pub const WM_KEY_DOWN: u32 = 1024 + 4; // WM_APP + 4, bind pressed (wparam=kind, lparam=code)
pub const WM_KEY_UP: u32 = 1024 + 5; // WM_APP + 5, bind released (wparam=kind, lparam=code)
pub const TIMER_POLL: usize = 1;

pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// True while one of apply_pos/apply_topmost/apply_lock below is running.
// Lets the WM_WINDOWPOSCHANGING guard tell our own geometry changes apart
// from foreign ones (other apps, snap commits outside drag loops, etc.).
static mut GEO_SELF: bool = false;

pub fn geo_self() -> bool {
    unsafe { GEO_SELF }
}

fn flagged<R>(f: impl FnOnce() -> R) -> R {
    unsafe {
        GEO_SELF = true;
    }
    let r = f();
    unsafe {
        GEO_SELF = false;
    }
    r
}

/// Strict sanity check per spec: on startup and on WM_DISPLAYCHANGE,
/// if the saved position is not fully inside the virtual screen,
/// reset to (40,40) on the primary monitor.
pub fn clamp_position(x: i32, y: i32, size: u32) -> (i32, i32) {
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if vw <= 0 || vh <= 0 {
            return (40, 40);
        }
        let s = size as i32;
        // If window bigger than virtual screen, reset to primary.
        if s > vw || s > vh {
            return (40, 40);
        }
        let inside = x >= vx
            && y >= vy
            && x + s <= vx + vw
            && y + s <= vy + vh;
        if inside {
            (x, y)
        } else {
            (40, 40)
        }
    }
}

/// Gentle variant for size changes: keep the current top-left and only
/// shift it the minimum needed to stay fully visible. Never teleports
/// across the screen (unlike clamp_position).
pub fn nudge_into_view(x: i32, y: i32, size: u32) -> (i32, i32) {
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if vw <= 0 || vh <= 0 {
            return (x, y);
        }
        let s = size as i32;
        if s > vw || s > vh {
            return (x, y);
        }
        (
            x.clamp(vx, vx + vw - s),
            y.clamp(vy, vy + vh - s),
        )
    }
}

pub fn apply_lock(hwnd: HWND, locked: bool) {
    flagged(|| unsafe {
        let idx = GWL_EXSTYLE;
        let ex = GetWindowLongPtrW(hwnd, idx) as u32;
        let new_ex = if locked {
            ex | WS_EX_TRANSPARENT.0
        } else {
            ex & !WS_EX_TRANSPARENT.0
        };
        if new_ex != ex {
            SetWindowLongPtrW(hwnd, idx, new_ex as isize);
            // Ex-style changes require FRAMECHANGED to apply immediately.
            // NOMOVE/NOSIZE/NOACTIVATE/NOZORDER: position, size, z-order
            // and activation are all preserved.
            let _ = SetWindowPos(
                hwnd,
                None,
                0, 0, 0, 0,
                SWP_NOMOVE
                    | SWP_NOSIZE
                    | SWP_NOACTIVATE
                    | SWP_NOZORDER
                    | SWP_FRAMECHANGED,
            );
        }
        // Redraw is handled by caller (needs outline change).
    })
}

pub fn apply_topmost(hwnd: HWND, on: bool) {
    flagged(|| unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(if on { HWND_TOPMOST } else { HWND_NOTOPMOST }),
            0, 0, 0, 0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    })
}

pub fn apply_pos(hwnd: HWND, x: i32, y: i32, size: u32) {
    flagged(|| unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            x, y,
            size as i32, size as i32,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    })
}

pub fn window_rect(hwnd: HWND) -> Option<(i32, i32)> {
    unsafe {
        let mut r = Default::default();
        if GetWindowRect(hwnd, &mut r).is_ok() {
            Some((r.left, r.top))
        } else {
            None
        }
    }
}
