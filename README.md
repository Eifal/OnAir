# OnAir

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

Requirements: Rust 1.85+ (stable), target `x86_64-pc-windows-msvc`, Windows 10/11.

```powershell
cargo build --release
# exe: target\release\onair.exe  (~190 KB; 193,536 bytes)
```

Release profile (`Cargo.toml`) uses `opt-level="z"`, `lto`, `panic="abort"`,
`strip` for minimal size. Idle CPU < 0.1% (single `GetMessageW` loop +
`SetTimer` polling, redraw only on change), working set ~ <10 MB.

The only runtime dependency is the `windows` crate; `winresource` is
build-only and embeds the app icon/version metadata. Config parsing is
hand-written `key=value`, Win32 API is used directly (no GUI framework),
`#![windows_subsystem = "windows"]` so there is no console window.

## Run

Just run the exe. First run:

1. Creates `config.ini` next to the exe (or `%APPDATA%\onair\config.ini`
   if the exe folder is not writable; an existing legacy
   `%APPDATA%\vm-mic-indicator\config.ini` is kept for upgrades).
2. Starts **unlocked** so you can drag it where you want.
3. Right-click / left-click the tray icon → check **Lock position**.

Single instance is enforced with a named mutex (`Local\OnAirOverlay`).

## Tray menu (the only UI)

Left and right click both open the menu:

- Lock position (checkmark)
- Always on top (checkmark, re-asserted ~every 2 s)
- Size: 24, 32, 48, 64, 96, 128 px
- Opacity: 100 … 20 % (whole window)
- Visual: Dot / Ring / Bar / Mini (see below)
- Background: 100 / 90 / 70 / 50 / 30 / Transparent (bg square only)
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
  - Trigger mode: Toggle (press) / Hold (push-to-talk) / Both
  - Hold tap threshold: 200 / 300 / 500 / 800 ms (Both mode only)

Every change applies immediately, saves `config.ini`, and redraws if needed.
Position is adopted on move and saved on drag end (`WM_EXITSIZEMOVE`) and exit.

## Config (`config.ini`)

`key=value`, one per line, `#` = comment, unknown keys ignored,
invalid values fall back to defaults. Written atomically on actual changes
and on exit.

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
trigger_mode=toggle
hold_tap_ms=300
visual_style=dot
bg_opacity=90
color_on=2ECC71
color_off=E74C3C
color_unknown=808080
color_bg=141414
```

Clamps: `size` 16–512, `opacity` 20–100, `poll_ms` 30–1000,
`hold_tap_ms` 100–2000, `bg_opacity` 0–100. Colors are 6-digit hex without `#`.
`trigger_mode` accepts `toggle` / `hold` / `both` (aliases: `press`,
`ptt`, `toggle+hold`); unknown falls back to `toggle`.
`visual_style` accepts `dot` / `ring` / `bar` / `mini`
(aliases: `strip`/`pill` → bar, `circle`/`outline` → ring,
`minimal`/`nobg` → mini); unknown falls back to `dot`.

## Visual styles (all small, same square hitbox)

- **Dot** — rounded square + filled circle (default, most visible).
- **Ring** — rounded square + hollow colored ring, dark center
  (less glare at night, still easy to read).
- **Bar** — rounded square + thin colored strip at the bottom
  (smallest color area, good for corner placement).
- **Mini** — no background, just a small dot (~54% size)
  (minimal footprint; use 32–48 px, 80–100% opacity).

Tray → **Visual** switches instantly and saves `config.ini`.
Tray icon always stays Dot for recognizability.

## Background transparency (`bg_opacity`)

- Only the rounded-square background fades; the colored
  dot/ring/strip stays fully opaque for readability.
- Global **Opacity** still applies on top of everything
  (e.g. `opacity=50` + `bg_opacity=50` = very faint bg).
- `bg_opacity=0` = no background at all (floating dot/ring/strip).
  Hitbox/drag area stays `size`×`size` — only the paint changes,
  click-through when locked is unaffected.
- No effect on **Mini** (already has no background).
- Tray → **Background** or `config.ini` (`bg_opacity`, aliases
  `bg_alpha`, `background_opacity`).

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

## Mic toggle / hold via keybind

The overlay can drive the configured Voicemeeter button itself, so
Voicemeeter's own macro button is optional. Tray → **Trigger mode**:

- **Toggle (press)** — key/button down flips the parameter permanently.
- **Hold (push-to-talk)** — while held the mic is forced ON (green),
  on release it returns to the state saved on key-down.
- **Both** — tap faster than `hold_tap_ms` toggles, holding longer is
  momentary (release restores the saved state).

Details:

- Tray → **Set toggle key…** → press a keyboard key or mouse button
  anywhere (Left/Right/Middle/X1/X2 side buttons). Modifier-only presses
  (Shift/Ctrl/Alt/Win) are ignored, so Ctrl+A binds A. Esc cancels, the
  capture also times out after 15 s, and opening the tray menu cancels it.
- The event is always **passed through**: the key keeps working normally in
  games/apps AND drives the mic.
- Keyboard auto-repeat is suppressed: one press = one down/up pair, holding
  never spams toggles. Key-up is only used by Hold/Both (ignored in Toggle).
- Hold writes the value that renders as ON (accounts for `invert`), so
  `invert=true` still gives green while held.
- If Voicemeeter is unreachable (gray state), holds/toggles are ignored.
  If you change the parameter manually mid-hold, the release restores the
  saved state and overwrites that manual change (expected for simple PTT).
- `hold_tap_ms` (Both only) is configurable via tray
  (200/300/500/800 ms) or freely in `config.ini` (100–2000, default 300).
- Coverage is whatever Windows low-level hooks see: keyboard incl. F13–F24,
  plus Mouse Left/Right/Middle/X1/X2. Extra vendor-only gaming buttons are
  invisible to Windows itself and cannot be bound.
- Technique: `WH_KEYBOARD_LL` + `WH_MOUSE_LL` on our own UI thread (same
  pattern as Discord push-to-talk) — no injection, no game memory access,
  no admin needed. Hooks are installed only while a bind exists or capture
  is active; if installation fails, the feature silently stays inactive.

## Safety / non-goals

- Separate top-most window only, plus low-level input hooks for the optional
  toggle keybind. No hooks into the game, no DLL injection,
  no game-memory reading.
- No sound, no clickable overlay, no settings window/dialogs, no separate
  hotkey driver or background service, no autostart, no auto-update, no
  telemetry, no log files. The optional keybind is handled in-process.
- Windows 10/11 64-bit only.

## Layout

```
Cargo.toml
build.rs       // embeds assets/onair.ico + version metadata
assets/
  onair.ico
LICENSE
src/
  main.rs      // entry, single instance, message loop, timers, WndProc
  window.rs    // helpers: lock / topmost / pos / clamp / drag via HTCAPTION
  render.rs    // DIB + SDF rasterizer + UpdateLayeredWindow + tray icon
  vm.rs        // dynamic DLL loading, login, polling, MicState
  tray.rs      // Shell_NotifyIcon, menu, commands, tooltip
  config.rs    // load / save / defaults / clamping
  hotkey.rs    // WH_KEYBOARD_LL + WH_MOUSE_LL mic toggle bind + capture
```
