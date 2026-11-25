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
//! Writing is asynchronous: callers (including the low-level hook procs) only
//! format a line and push it onto a bounded queue. A dedicated thread owns the
//! file, so a slow disk can never stall the hook thread — which would make
//! Windows drop the hook. When the queue or the size cap is reached, lines are
//! dropped instead of blocking.
//!
//! The log is capped at 4 MiB per run and then stops, so a forgotten flag can
//! never fill the disk.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Anything slower than this is written to the log. Normal Remote API calls
/// are microseconds; a value in the hundreds of ms means the UI thread stalled.
pub const SLOW_MS: u128 = 25;

/// Hard cap for one run. Polling is not logged per tick, so this is generous.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// The UI thread is considered stalled after this long without a message.
const STALL_MS: u64 = 3000;

/// Pending lines kept in memory. ~1 MiB of text; beyond this lines are
/// dropped rather than blocking the caller.
const QUEUE_LINES: usize = 8192;

/// How long `flush()` waits for the writer to catch up at exit.
const FLUSH_WAIT: Duration = Duration::from_millis(500);

enum Msg {
    Line(String),
    Flush(std::sync::mpsc::Sender<()>),
    Stop,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static START: OnceLock<Instant> = OnceLock::new();
static TX: OnceLock<SyncSender<Msg>> = OnceLock::new();
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
    let (tx, rx) = sync_channel(QUEUE_LINES);
    if std::thread::Builder::new()
        .name("onair-log".to_string())
        .spawn(move || writer(rx, file))
        .is_err()
    {
        return;
    }
    let _ = TX.set(tx);
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

/// Owns the file for the whole run. Writes and flushes there so no caller,
/// especially a low-level hook proc, ever waits for the disk.
fn writer(rx: Receiver<Msg>, file: File) {
    let mut file = file;
    let mut bytes: u64 = 0;
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Line(line) => {
                if bytes >= MAX_BYTES {
                    continue;
                }
                let dropped = DROPPED.swap(0, Ordering::Relaxed);
                if dropped > 0 {
                    let note = format!(
                        "[{:>8}ms] ... {dropped} log lines dropped (writer behind)\n",
                        elapsed_ms()
                    );
                    let _ = file.write_all(note.as_bytes());
                    bytes += note.len() as u64;
                }
                let _ = file.write_all(line.as_bytes());
                let _ = file.flush();
                bytes += line.len() as u64;
                if bytes >= MAX_BYTES {
                    let _ = file.write_all(b"=== log cap reached, logging stopped ===\n");
                    let _ = file.flush();
                    ENABLED.store(false, Ordering::Relaxed);
                }
            }
            Msg::Flush(ack) => {
                let _ = file.flush();
                let _ = ack.send(());
            }
            Msg::Stop => break,
        }
    }
    let _ = file.flush();
}

/// Write one line. No-op unless enabled; never blocks.
pub fn write(args: std::fmt::Arguments<'_>) {
    if !enabled() {
        return;
    }
    let Some(tx) = TX.get() else {
        return;
    };
    let mut line = String::with_capacity(96);
    use std::fmt::Write as _;
    let _ = write!(line, "[{:>8}ms] ", elapsed_ms());
    let _ = line.write_fmt(args);
    line.push('\n');
    match tx.try_send(Msg::Line(line)) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}

/// Wait for the writer to catch up (exit path only, bounded).
pub fn flush() {
    let Some(tx) = TX.get() else {
        return;
    };
    let (ack_tx, ack_rx) = std::sync::mpsc::channel();
    if tx.try_send(Msg::Flush(ack_tx)).is_ok() {
        let _ = ack_rx.recv_timeout(FLUSH_WAIT);
    }
}

/// Flush and stop the writer thread.
pub fn shutdown() {
    flush();
    if let Some(tx) = TX.get() {
        let _ = tx.try_send(Msg::Stop);
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
                std::thread::sleep(Duration::from_millis(500));
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
