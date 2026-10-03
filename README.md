# VM Mic Indicator

Tiny Windows overlay showing whether one Voicemeeter parameter is ON or OFF.

- 🟢 green circle = mic live (ON)
- 🔴 red circle = mic muted (OFF)
- ⚪ gray circle = cannot be read (Voicemeeter closed / bad parameter / not installed)

Default parameter: **Strip 1, button B2** (`Strip[0].B2`). Toggle B2 with a
Voicemeeter macro button (e.g. mouse side button) to mute/unmute.
Overlay is meant for FiveM / GTA RP in **borderless windowed** mode.

No focus stealing, no clicks captured when locked, near-zero CPU/RAM.

## Known limitation

The overlay **does not show over games in true exclusive fullscreen**.
Set FiveM / GTA V to **Borderless / Windowed**. This is expected (top-most
layered windows are not composited over exclusive fullscreen), not a bug.

## Build

Requirements: Rust stable, target `x86_64-pc-windows-msvc`, Windows 10/11.

```powershell
cargo build --release
# exe: target\release\onair.exe  (~160 KB)
```

Release profile (`Cargo.toml`) uses `opt-level="z"`, `lto`, `panic="abort"`,
`strip` for minimal size. Idle CPU < 0.1% (single `GetMessageW` loop +
`SetTimer` polling, redraw only on change), working set ~ <10 MB.

No other dependencies besides the `windows` crate. Config parsing is
hand-written `key=value`, Win32 API is used directly (no GUI framework),
`#![windows_subsystem = "windows"]` so there is no console window.

## Run

Just run the exe. First run:

1. Creates `config.ini` next to the exe (or `%APPDATA%\vm-mic-indicator\config.ini`
   if the exe folder is not writable).
2. Starts **unlocked** so you can drag it where you want.
3. Right-click / left-click the tray icon → check **Lock position**.

Single instance is enforced with a named mutex (`Local\VMMicIndicator`).

## Tray menu (the only UI)

Left and right click both open the menu:

- Lock position (checkmark)
- Always on top (checkmark, re-asserted ~every 2 s)
- Size: 24, 32, 48, 64, 96, 128 px
- Opacity: 100 … 20 %
- Voicemeeter source:
  - Strip 1–8 (→ `Strip[n-1]`)
  - Button A1–A5, B1–B3, Mute
  - Invert (checkmark)
  - If `vm_param` was hand-edited to something else, a disabled
    `Custom: <param>` label is shown at the top of this submenu.
- Open config file / Reload config / Quit
- Mic toggle keybind (see below):
  - `Toggle key: <name>` (current bind, informational)
  - Set toggle key… / Clear toggle key

Every change applies immediately, saves `config.ini`, and redraws if needed.
Position is saved on drag end (`WM_EXITSIZEMOVE`).

## Config (`config.ini`)

`key=value`, one per line, `#` = comment, unknown keys ignored,
invalid values fall back to defaults. Written only on actual change.

```
x=40
y=40
size=48
opacity=90
locked=false
always_on_top=true
vm_param=Strip[0].B2
invert=false
poll_ms=100
toggle_key=None
color_on=2ECC71
color_off=E74C3C
color_unknown=808080
color_bg=141414
```

Clamps: `size` 16–512, `opacity` 20–100, `poll_ms` 30–1000.
Colors are 6-digit hex without `#`.

- `vm_param` is zero-indexed: UI "Strip 1" = `Strip[0]`.
  Examples: `Strip[0].B2`, `Strip[0].Mute`, `Bus[1].Mute`.
- `invert=true` flips green/red, so parameters with opposite logic
  (e.g. `Strip[0].Mute = 1` means muted) work correctly.

## Voicemeeter

Uses the official **Voicemeeter Remote API** (`VoicemeeterRemote64.dll`):

- Loaded at runtime with `LoadLibraryW` + `GetProcAddress` (`extern "system"`).
  Never linked statically.
- DLL path: registry `HKLM\SOFTWARE\WOW6432Node\...\Uninstall\VB:Voicemeeter {17359A74-1236-5467}`
  → `UninstallString` directory + `VoicemeeterRemote64.dll`,
  fallback `C:\Program Files (x86)\VB\Voicemeeter\VoicemeeterRemote64.dll`.
- Functions: `VBVMR_Login` (0 OK, 1 OK but app not running),
  `VBVMR_Logout`, `VBVMR_IsParametersDirty`, `VBVMR_GetParameterFloat(name, &mut value)`
  (0 OK, -2 no server, -3 unknown parameter). Verified against the
  VB-Audio "Voicemeeter Remote API" PDF + `VoicemeeterRemote.h`.
- Polling every `poll_ms` (default 100 ms): login at most once/sec when
  disconnected, dirty-check (best effort), `GetParameterFloat`,
  `on = value >= 0.5` + `invert`. Only redraws / updates tray on change.
- Handles Voicemeeter started after the overlay, closed and restarted —
  recovers automatically (gray while closed, no crash, no popups).
- Writing back: `VBVMR_SetParameterFloat(name, value)`
  (0 OK, -2 no server, -3 unknown parameter) powers the toggle keybind.

## Mic toggle via keybind

The overlay can flip the configured Voicemeeter button itself: it reads the
current value and writes back the inverse, so Voicemeeter's own macro button
is optional.

- Tray → **Set toggle key…** → press any keyboard key or mouse button
  anywhere (Left/Right/Middle/X1/X2 side buttons). Esc cancels, the capture
  also times out after 15 s, and opening the tray menu cancels it.
- The event is always **passed through**: the key keeps working normally in
  games/apps AND toggles the mic.
- Only key-down/button-down fires (holding does not spam, key-up ignored).
- Coverage is whatever Windows low-level hooks see: keyboard incl. F13–F24,
  plus Mouse Left/Right/Middle/X1/X2. Extra vendor-only gaming buttons are
  invisible to Windows itself and cannot be bound.
- Technique: `WH_KEYBOARD_LL` + `WH_MOUSE_LL` on our own UI thread (same
  pattern as Discord push-to-talk) — no injection, no game memory access,
  no admin needed. If hook installation ever fails, the feature silently
  stays inactive.

## Safety / non-goals

- Separate top-most window only, plus low-level input hooks for the optional
  toggle keybind. No hooks into the game, no DLL injection,
  no game-memory reading.
- No sound, no clickable overlay, no settings window/dialogs, no hotkeys,
  no autostart, no auto-update, no telemetry, no log files.
- Windows 10/11 64-bit only.

## Layout

```
Cargo.toml
src/
  main.rs      // entry, single instance, message loop, timers, WndProc
  window.rs    // helpers: lock / topmost / pos / clamp / drag via HTCAPTION
  render.rs    // DIB + SDF rasterizer + UpdateLayeredWindow + tray icon
  vm.rs        // dynamic DLL loading, login, polling, MicState
  tray.rs      // Shell_NotifyIcon, menu, commands, tooltip
  config.rs    // load / save / defaults / clamping
  hotkey.rs    // WH_KEYBOARD_LL + WH_MOUSE_LL mic toggle bind + capture
```
