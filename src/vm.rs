use windows::core::PCWSTR;
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Registry::*;

// Verified against official VB-Audio "Voicemeeter Remote API" doc + VoicemeeterRemote.h:
//   long __stdcall VBVMR_Login(void);            // 0 OK, 1 OK but app not running, <0 error
//   long __stdcall VBVMR_Logout(void);           // 0 ok
//   long __stdcall VBVMR_IsParametersDirty(void);// 0 no change, 1 changed, -1 err, -2 no server
//   long __stdcall VBVMR_GetParameterFloat(char*, float*); // 0 ok, -1 err, -2 no server, -3 unknown, -5 mismatch
//   long __stdcall VBVMR_SetParameterFloat(char*, float);  // 0 ok, -1 err, -2 no server, -3 unknown, -5 mismatch
type FnLogin = unsafe extern "system" fn() -> i32;
type FnLogout = unsafe extern "system" fn() -> i32;
type FnDirty = unsafe extern "system" fn() -> i32;
type FnGetFloat = unsafe extern "system" fn(*mut u8, *mut f32) -> i32;
type FnSetFloat = unsafe extern "system" fn(*mut u8, f32) -> i32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicState {
    On,
    Off,
    Unknown(String),
}

pub struct VmClient {
    lib: Option<HMODULE>,
    f_login: Option<FnLogin>,
    f_logout: Option<FnLogout>,
    f_dirty: Option<FnDirty>,
    f_get: Option<FnGetFloat>,
    f_set: Option<FnSetFloat>,
    pub logged_in: bool,
    last_login: Option<std::time::Instant>,
    last_load_attempt: Option<std::time::Instant>,
    pub dll_missing: bool,
}

/// How often to retry registry/path/LoadLibrary when Voicemeeter's DLL is
/// absent. Polling still runs at `poll_ms`, but disk/registry work is
/// throttled to once per this interval.
const LOAD_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

impl VmClient {
    pub fn new() -> Self {
        let mut c = Self {
            lib: None,
            f_login: None,
            f_logout: None,
            f_dirty: None,
            f_get: None,
            f_set: None,
            logged_in: false,
            last_login: None,
            last_load_attempt: None,
            dll_missing: false,
        };
        c.try_load();
        c
    }

    fn try_load(&mut self) {
        self.last_load_attempt = Some(std::time::Instant::now());
        let Some(path) = find_dll_path() else {
            self.dll_missing = true;
            return;
        };
        unsafe {
            let w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let h = LoadLibraryW(PCWSTR(w.as_ptr()));
            let Ok(h) = h else {
                self.dll_missing = true;
                return;
            };
            // GetProcAddress takes PCSTR (ANSI)
            let login = GetProcAddress(h, windows::core::s!("VBVMR_Login"));
            let logout = GetProcAddress(h, windows::core::s!("VBVMR_Logout"));
            let dirty = GetProcAddress(h, windows::core::s!("VBVMR_IsParametersDirty"));
            let getf = GetProcAddress(h, windows::core::s!("VBVMR_GetParameterFloat"));
            let (Some(login), Some(logout), Some(dirty), Some(getf)) = (login, logout, dirty, getf)
            else {
                let _ = FreeLibrary(h);
                self.dll_missing = true;
                return;
            };
            self.f_login = Some(std::mem::transmute::<*const (), FnLogin>(
                login as *const (),
            ));
            self.f_logout = Some(std::mem::transmute::<*const (), FnLogout>(
                logout as *const (),
            ));
            self.f_dirty = Some(std::mem::transmute::<*const (), FnDirty>(
                dirty as *const (),
            ));
            self.f_get = Some(std::mem::transmute::<*const (), FnGetFloat>(
                getf as *const (),
            ));
            // SetParameterFloat is optional: older DLLs always have it, but a
            // missing setter must only disable toggling, never the display.
            if let Some(setf) = GetProcAddress(h, windows::core::s!("VBVMR_SetParameterFloat")) {
                self.f_set = Some(std::mem::transmute::<*const (), FnSetFloat>(
                    setf as *const (),
                ));
            }
            self.lib = Some(h);
            self.dll_missing = false;
        }
    }

    fn ensure_loaded(&mut self) {
        if self.lib.is_none() {
            let due = self
                .last_load_attempt
                .map(|t| t.elapsed() >= LOAD_RETRY)
                .unwrap_or(true);
            if due {
                self.try_load();
            }
        }
    }

    /// Poll once. Returns new MicState.
    pub fn poll(&mut self, vm_param: &str, invert: bool) -> MicState {
        self.ensure_loaded();
        if self.dll_missing || self.f_login.is_none() {
            self.logged_in = false;
            return MicState::Unknown("Voicemeeter not installed".to_string());
        }
        let (f_login, f_dirty, f_get) = match (self.f_login, self.f_dirty, self.f_get) {
            (Some(a), Some(b), Some(c)) => (a, b, c),
            _ => return MicState::Unknown("Voicemeeter not installed".to_string()),
        };

        // Login at most once per second when not logged in.
        if !self.logged_in {
            let now = std::time::Instant::now();
            if let Some(last) = self.last_login {
                if now.duration_since(last).as_millis() < 1000 {
                    return MicState::Unknown("Voicemeeter not running".to_string());
                }
            }
            self.last_login = Some(now);
            let rep = unsafe { f_login() };
            if rep == 0 || rep == 1 {
                self.logged_in = true;
                // rep==1 means app not running yet; still try reading (will report no server).
            } else {
                self.logged_in = false;
                return MicState::Unknown("Voicemeeter not running".to_string());
            }
        }

        // Best-effort dirty check; ignore result except errors.
        let dirty = unsafe { f_dirty() };
        if dirty == -2 {
            // no server -> logout so we retry login later (throttled)
            self.do_logout();
            return MicState::Unknown("Voicemeeter not running".to_string());
        }

        // Read parameter (ANSI null-terminated)
        let mut name: Vec<u8> = vm_param.as_bytes().to_vec();
        name.push(0);
        let mut val: f32 = 0.0;
        let rep = unsafe { f_get(name.as_mut_ptr(), &mut val as *mut f32) };
        match rep {
            0 => {
                let mut on = val >= 0.5;
                if invert {
                    on = !on;
                }
                if on {
                    MicState::On
                } else {
                    MicState::Off
                }
            }
            -2 => {
                self.do_logout();
                MicState::Unknown("Voicemeeter not running".to_string())
            }
            -3 => MicState::Unknown("Unknown parameter".to_string()),
            _ => MicState::Unknown("Voicemeeter error".to_string()),
        }
    }

    fn do_logout(&mut self) {
        if self.logged_in {
            if let Some(f) = self.f_logout {
                unsafe {
                    let _ = f();
                }
            }
        }
        self.logged_in = false;
    }

    /// One immediate login attempt (no throttle). Returns true when the
    /// communication pipe is open afterwards.
    fn try_login_now(&mut self) -> bool {
        let Some(f_login) = self.f_login else {
            return false;
        };
        self.last_login = Some(std::time::Instant::now());
        let rep = unsafe { f_login() };
        if rep == 0 || rep == 1 {
            self.logged_in = true;
            true
        } else {
            self.logged_in = false;
            false
        }
    }

    fn ansi_param(vm_param: &str) -> Vec<u8> {
        let mut name: Vec<u8> = vm_param.as_bytes().to_vec();
        name.push(0);
        name
    }

    /// User-initiated login with a short throttle (for keybind actions).
    /// Returns true when the pipe is open afterwards.
    fn ensure_login_for_action(&mut self) -> bool {
        self.ensure_loaded();
        if self.dll_missing || self.f_login.is_none() {
            return false;
        }
        if self.logged_in {
            return true;
        }
        let recent = self
            .last_login
            .map(|t| std::time::Instant::now().duration_since(t).as_millis() < 300)
            .unwrap_or(false);
        if recent {
            return false;
        }
        self.try_login_now()
    }

    /// Write a raw float value. Returns true on success, false silently
    /// on any failure (server gone -> pipe dropped for poll() to retry).
    pub fn set_raw(&mut self, vm_param: &str, value: f32) -> bool {
        if !self.ensure_login_for_action() {
            return false;
        }
        let f_set = match self.f_set {
            Some(f) => f,
            None => return false,
        };
        // Need a valid read path too (else DLL half-loaded).
        if self.f_get.is_none() {
            return false;
        }
        let mut name = Self::ansi_param(vm_param);
        let rep = unsafe { f_set(name.as_mut_ptr(), value) };
        if rep == -2 {
            self.do_logout();
            return false;
        }
        rep == 0
    }

    /// Flip a 0/1 button parameter (e.g. Strip[0].B2): reads the current
    /// value and writes back the inverse. Silent on any failure; the next
    /// poll() shows the resulting state. Toggle is symmetric, so `invert`
    /// (display-only) is intentionally ignored here.
    pub fn toggle_param(&mut self, vm_param: &str) {
        self.ensure_loaded();
        if self.dll_missing {
            return;
        }
        let (f_get, f_set) = match (self.f_get, self.f_set) {
            (Some(g), Some(s)) => (g, s),
            _ => return,
        };
        if !self.logged_in {
            // User-initiated: allow a quicker retry than poll's 1s
            // throttle, but still guard against hammering.
            let recent = self
                .last_login
                .map(|t| std::time::Instant::now().duration_since(t).as_millis() < 300)
                .unwrap_or(false);
            if recent || !self.try_login_now() {
                return;
            }
        }
        let mut name = Self::ansi_param(vm_param);
        let mut val: f32 = 0.0;
        if unsafe { f_get(name.as_mut_ptr(), &mut val as *mut f32) } != 0 {
            return;
        }
        let flipped = if val >= 0.5 { 0.0 } else { 1.0 };
        if unsafe { f_set(name.as_mut_ptr(), flipped) } == -2 {
            // Server went away mid-toggle: drop the pipe so poll()
            // retries the login later instead of erroring forever.
            self.do_logout();
        }
    }
    /// Explicitly close the Voicemeeter session and unload the DLL.
    /// Idempotent; used on shutdown because `APP` is a static and its
    /// `Drop` impl never runs at process exit.
    pub fn shutdown(&mut self) {
        self.do_logout();
        if let Some(h) = self.lib.take() {
            unsafe {
                let _ = FreeLibrary(h);
            }
        }
        self.f_login = None;
        self.f_logout = None;
        self.f_dirty = None;
        self.f_get = None;
        self.f_set = None;
        self.last_login = None;
    }
}

impl Drop for VmClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Locate VoicemeeterRemote64.dll via registry, fallback to default path.
pub fn find_dll_path() -> Option<String> {
    // Registry: HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\VB:Voicemeeter {17359A74-1236-5467}
    // value: UninstallString
    if let Some(dir) = read_uninstall_dir() {
        let cand = format!(
            "{}\\VoicemeeterRemote64.dll",
            dir.trim_end_matches(['\\', '/'])
        );
        if std::path::Path::new(&cand).exists() {
            return Some(cand);
        }
    }
    let fallback = r"C:\Program Files (x86)\VB\Voicemeeter\VoicemeeterRemote64.dll";
    if std::path::Path::new(fallback).exists() {
        return Some(fallback.to_string());
    }
    None
}

fn read_uninstall_dir() -> Option<String> {
    unsafe {
        let subkey: Vec<u16> = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\VB:Voicemeeter {17359A74-1236-5467}"
            .encode_utf16().chain(std::iter::once(0)).collect();
        let value: Vec<u16> = "UninstallString"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut hkey = HKEY::default();
        let rc = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            Some(0),
            KEY_READ,
            &mut hkey,
        );
        if rc.is_err() {
            return None;
        }
        let mut ty: REG_VALUE_TYPE = REG_NONE;
        let mut size: u32 = 0;
        // Query size first
        let _ = RegQueryValueExW(
            hkey,
            PCWSTR(value.as_ptr()),
            None,
            Some(&mut ty),
            None,
            Some(&mut size),
        );
        if size == 0 || size > 32768 {
            let _ = RegCloseKey(hkey);
            return None;
        }
        let mut buf = vec![0u16; (size as usize / 2) + 2];
        let mut size2 = (buf.len() * 2) as u32;
        let rc = RegQueryValueExW(
            hkey,
            PCWSTR(value.as_ptr()),
            None,
            Some(&mut ty),
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut size2),
        );
        let _ = RegCloseKey(hkey);
        if rc.is_err() {
            return None;
        }
        // buf is UTF-16, may contain quotes + exe name + args. Extract directory.
        let s = String::from_utf16_lossy(&buf);
        let s = s.trim_matches('\0').trim().trim_matches('"').to_string();
        // UninstallString is like: "C:\Program Files (x86)\VB\Voicemeeter\uninst.exe"
        // Take parent dir.
        let path = std::path::Path::new(&s);
        if let Some(parent) = path.parent() {
            return Some(parent.to_string_lossy().to_string());
        }
        None
    }
}
