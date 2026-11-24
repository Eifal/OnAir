//! Opt-in diagnostics (`debug_log=true` in config.ini, or `ONAIR_DEBUG=1`).
//!
//! OnAir is a quiet tray app by design, so this module does nothing unless the
//! flag is set: every call site is a relaxed atomic load. When enabled it
//! writes `onair-debug.log` next to `config.ini` (previous run kept as
//! `onair-debug.old.log`) with:
//!
//! - every trigger event seen by the low-level hooks, including the
//!   `LLKHF_INJECTED` flag and the OS event time (catches phantom presses and
//!   distinguishes hardware chatter from software-injected duplicates),
//! - every Voicemeeter write with the value read before/after it,
//! - slow Voicemeeter/Shell calls (a blocked UI thread is the only way this
//!   app can "hang"),
//! - a watchdog thread that logs when the UI thread stops pumping messages,
//!   including which operation it was stuck in.
//!
//! The log is capped at 4 MiB per run and then stops, so a forgotten flag can
//! never fill the disk.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Anything slower than this is written to the log. Normal Remote API calls
/// are microseconds; a value in the hundreds of ms means the UI thread stalled.
pub const SLOW_MS: u128 = 25;

/// Hard cap for one run. Polling is not logged per tick, so this is generous.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// The UI thread is considered stalled after this long without a message.
const STALL_MS: u64 = 3000;

static ENABLED: AtomicBool = AtomicBool::new(false);
static BYTES: AtomicU64 = AtomicU64::new(0);
static START: OnceLock<Instant> = OnceLock::new();
static LOG: OnceLock<Mutex<File>> = OnceLock::new();
static HEARTBEAT: AtomicU64 = AtomicU64::new(0);
static STAGE: AtomicU8 = AtomicU8::new(STAGE_IDLE);
/// True while the UI thread sits in a blocking message wait. Waiting for input
/// is the healthy idle state, not a stall, so the watchdog ignores it.
static WAITING: AtomicBool = AtomicBool::new(false);

pub const STAGE_IDLE: u8 = 0;
pub const STAGE_POLL: u8 = 1;
pub const STAGE_ACTION: u8 = 2;
pub const STAGE_MENU: u8 = 3;

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Mark the current UI-thread operation so the watchdog can name it.
pub fn set_stage(stage: u8) {
    if enabled() {
        STAGE.store(stage, Ordering::Relaxed);
    }
}

pub fn stage_name() -> &'static str {
    match STAGE.load(Ordering::Relaxed) {
        STAGE_POLL => "poll",
        STAGE_ACTION => "action",
        STAGE_MENU => "menu",
        _ => "idle",
    }
}

/// Liveness signal from the message pump.
pub fn beat() {
    if enabled() {
        HEARTBEAT.store(elapsed_ms(), Ordering::Relaxed);
    }
}

/// Tell the watchdog whether the UI thread is idle in `GetMessageW` (healthy)
/// or busy handling a message (a long gap there is a real stall).
pub fn waiting(on: bool) {
    if enabled() {
        WAITING.store(on, Ordering::Relaxed);
    }
}

fn elapsed_ms() -> u64 {
    START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

/// Log file for the config next to `config.ini`.
pub fn log_path(config: &Path) -> PathBuf {
    config
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("onair-debug.log")
}

fn rotated(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "onair-debug".to_string());
    let ext = path
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    path.with_file_name(format!("{stem}.old{ext}"))
}

/// Enable logging and write the run header. Safe to call once at startup.
pub fn init(enable: bool, config: &Path) {
    if !enable {
        return;
    }
    let path = log_path(config);
    // Keep exactly one previous run for comparison, then start fresh.
    let old = rotated(&path);
    let _ = std::fs::remove_file(&old);
    if path.exists() {
        let _ = std::fs::rename(&path, &old);
    }
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = LOG.set(Mutex::new(file));
    START.get_or_init(Instant::now);
    ENABLED.store(true, Ordering::Relaxed);
    write(format_args!(
        "=== OnAir {} debug log ===\npid={} exe={} config={}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        config.display()
    ));
    start_watchdog();
}

/// Write one line. No-op unless enabled and the cap has not been reached.
pub fn write(args: std::fmt::Arguments<'_>) {
    if !enabled() {
        return;
    }
    let Some(lock) = LOG.get() else {
        return;
    };
    let mut line = format!("[{:>8}ms] ", elapsed_ms());
    use std::fmt::Write as _;
    let _ = line.write_fmt(args);
    line.push('\n');
    let Ok(mut file) = lock.lock() else {
        return;
    };
    let used = BYTES.load(Ordering::Relaxed);
    if used >= MAX_BYTES {
        ENABLED.store(false, Ordering::Relaxed);
        return;
    }
    let _ = file.write_all(line.as_bytes());
    let _ = file.flush();
    BYTES.store(used + line.len() as u64, Ordering::Relaxed);
    if used + line.len() as u64 >= MAX_BYTES {
        let _ = file.write_all(b"=== log cap reached, logging stopped ===\n");
        let _ = file.flush();
        ENABLED.store(false, Ordering::Relaxed);
    }
}

/// Log only calls that blocked the caller for `SLOW_MS` or more.
pub fn slow(op: &str, ms: u128, detail: impl std::fmt::Display) {
    if enabled() && ms >= SLOW_MS {
        write(format_args!("SLOW {op} {ms}ms ({detail})"));
    }
}

fn start_watchdog() {
    let _ = std::thread::Builder::new()
        .name("onair-watchdog".to_string())
        .spawn(|| {
            let mut stalled = false;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if !enabled() {
                    return;
                }
                let now = elapsed_ms();
                let beat = HEARTBEAT.load(Ordering::Relaxed);
                let lag = now.saturating_sub(beat);
                let waiting = WAITING.load(Ordering::Relaxed);
                if lag >= STALL_MS && !waiting {
                    if !stalled {
                        stalled = true;
                        write(format_args!(
                            "WATCHDOG ui thread stalled for {lag}ms, stage={}",
                            stage_name()
                        ));
                    }
                } else if stalled && lag < STALL_MS {
                    stalled = false;
                    write(format_args!("WATCHDOG ui thread resumed"));
                }
            }
        });
}

#[macro_export]
macro_rules! dbglog {
    ($($arg:tt)*) => {
        if $crate::debuglog::enabled() {
            $crate::debuglog::write(format_args!($($arg)*));
        }
    };
}
