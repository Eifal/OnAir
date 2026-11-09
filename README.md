# OnAir

Tiny Windows overlay showing whether a Voicemeeter button is ON or OFF, or
which of two buttons is currently active.

- 🟢 green circle = primary button live (ON)
- 🔴 red circle = primary button muted (OFF)
- ⚪ gray circle = cannot be read (Voicemeeter closed / bad parameter / not installed)

Default parameter: **Strip 1, button B2** (`Strip[0].B2`). Toggle B2 with a
Voicemeeter macro button (e.g. mouse side button) to mute/unmute, or use
**Action mode → Switch** to swap between two freely chosen buttons.
Overlay is meant for FiveM / GTA RP in **borderless windowed** mode.

No focus stealing, no clicks captured when locked, near-zero CPU/RAM.

## Known limitation

The overlay **does not show over games in true exclusive fullscreen**.
Set FiveM / GTA V to **Borderless / Windowed**. This is expected (top-most
layered windows are not composited over exclusive fullscreen), not a bug.

## Build

Requirements: Rust 1.85+ (stable), target `x86_64-pc-windows-msvc`,
Windows 10/11. The overlay keeps that MSRV; the settings window needs a
newer toolchain (Slint requires Rust 1.92).

```powershell
cargo build --release -p onair            # overlay   -> target\release\onair.exe
cargo build --release -p onair-settings   # settings  -> target\release\onair-settings.exe
# or build both:
cargo build --release --workspace         # both binaries
```

Both binaries ship **next to each other**; the settings app is launched
from the tray. The overlay never links Slint.

Sizes: `onair.exe` **259,584 bytes** (~254 KB). The settings exe cannot be
meaningfully smaller (~8 MB) because of the UI toolkit.

Release profile (`Cargo.toml`) uses `opt-level="z"`, `lto`, `panic="abort"`,
`strip` for minimal size. Idle CPU < 0.1% (single `GetMessageW` loop +
`SetTimer` polling, redraw only on change); working set ~15 MB for the
overlay, ~33 MB for the settings window while it is open.

The only overlay runtime dependency is the `windows` crate; shared config
logic lives in `onair-config` (plain Rust, no extra deps). `winresource`
build-only embeds the app icon/version metadata. The UI toolkit of the
settings window is Slint 1.18 with the winit backend and software
renderer; the overlay itself uses no GUI framework.

## Run

Just run the exe. First run:

1. Creates `config.ini` next to the exe (or `%APPDATA%\onair\config.ini`
   if the exe folder is not writable; an existing legacy
   `%APPDATA%\vm-mic-indicator\config.ini` is kept for upgrades).
2. Starts **unlocked** so you can drag it where you want.
3. Left-click the tray icon to open **OnAir Settings**, or right-click
   and choose it from the menu.

Single instance is enforced with a named mutex (`Local\OnAirOverlay`).
Settings is likewise single-instance (`Local\OnAirSettings`).

## Tray menu

Right-click the tray icon. Left click (or double-click) opens
**OnAir Settings** directly instead:

- Settings... (launches `onair-settings.exe` next to `onair.exe`)
- Lock position (checkmark)
- Always on top (checkmark, re-asserted ~every 2 s)
- Quit

Everything else lives in the Settings window; the tray menu stays a
quick stop, not a control panel. Changes made in Settings are written to
`config.ini` and picked up by the running overlay through a registered
window message (`OnAir.ReloadConfig`); edits made while OnAir is not
running apply on the next launch. Both EXEs also keep working if the
config file is hand-edited: Settings re-reads it immediately before
every write, so values the overlay owns (`x`, `y`, `locked`), comments
and unknown keys are never clobbered.

## OnAir Settings (the control window)

A separate executable (`onair-settings.exe`) with a full settings UI.
Solid dark (default) or light theme following the Windows app theme;
no Mica/acrylic/blur on the window surface itself. Native frame with a
dark title bar and rounded corners (Windows 11; DWM attributes are
best-effort elsewhere). Segoe UI Variable with Segoe UI fallback.
Keyboard accessible; 100/125/150 % DPI clean.

Pages:

- **General** – size, opacity, lock position, always on top, tray menu
  style (Glass acrylic / Native Win32).
- **Appearance** – visual style (Dot/Ring/Bar/Mini), background opacity,
  colours (on/off/unknown/background) with hex validation or a click-to-open
  HSV colour wheel (hue/saturation disc + value slider, Reset to defaults),
  and a live on-screen preview with a pulse animation.
- **Voicemeeter** – toggle vs two-button switch mode, primary/secondary
  parameter with strip/button dropdowns or a raw "Custom" value, invert,
  poll interval.
- **Hotkey** – capture the global toggle key (the overlay installs the
  low-level hook; Settings just asks it to and polls `config.ini`),
  trigger mode and hold/tap threshold.
- **About** – version, config file / folder open, privacy statement,
  Slint attribution.

All changes are debounced (~150 ms) for sliders/text and immediate for
switches/segmented controls; a toast confirms writes. If the overlay is
not running, a subtle banner says edits are saved for the next launch.

## Config (`config.ini`)

`key=value`, one per line, `#` = comment, unknown keys ignored,
invalid values fall back to defaults. Written atomically on actual changes
and on exit. The Settings window edits the same file with line-based
rewrites, preserving comments, unknown keys and ordering.

```
x=40
y=40
size=48
opacity=90
locked=false
always_on_top=true
vm_mode=toggle
vm_param=Strip[0].B2
vm_param_b=Strip[0].B1
invert=false
poll_ms=100
toggle_key=None
trigger_mode=toggle
hold_tap_ms=300
visual_style=dot
bg_opacity=90
menu_style=glass
color_on=2ECC71
color_off=E74C3C
color_unknown=808080
color_bg=141414
```

Clamps: `size` 16–512, `opacity` 20–100, `poll_ms` 30–1000,
`hold_tap_ms` 100–2000, `bg_opacity` 0–100. Colors are 6-digit hex without `#`.
`trigger_mode` accepts `toggle` / `hold` / `both` (aliases: `press`,
`ptt`, `toggle+hold`); unknown falls back to `toggle`.
`vm_mode` accepts `toggle` / `switch` (aliases: `swap`, `a/b`, `alternate`,
`two buttons`); unknown falls back to `toggle`.
`visual_style` accepts `dot` / `ring` / `bar` / `mini`
(aliases: `strip`/`pill` → bar, `circle`/`outline` → ring,
`minimal`/`nobg` → mini); unknown falls back to `dot`.
`menu_style` accepts `glass` / `native` (aliases: `acrylic`/`blur`/`modern`
→ glass, `classic`/`win32`/`system` → native); unknown falls back to `glass`.

## Visual styles (all small, same square hitbox)

- **Dot** — rounded square + filled circle (default, most visible).
- **Ring** — rounded square + hollow colored ring, dark center
  (less glare at night, still easy to read).
- **Bar** — rounded square + thin colored strip at the bottom
  (smallest color area, good for corner placement).
- **Mini** — no background, just a small dot (~54% size)
  (minimal footprint; use 32–48 px, 80–100% opacity).

Settings → **Appearance → Visual** switches immediately and saves `config.ini`.
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
- Settings → **Appearance → Background** or `config.ini` (`bg_opacity`, aliases
  `bg_alpha`, `background_opacity`).

## Tray menu style

- `menu_style=glass` (default) draws the custom rounded acrylic menu.
  If acrylic is unavailable, it falls back to a solid dark translucent panel.
  Windows 11 uses smooth DWM rounded corners; older Windows falls back to a
  clipped rounded region. No drop shadow is drawn.
- `menu_style=native` keeps the exact Win32 system menu, which is also the
  accessibility fallback. The glass menu falls back to it automatically if
  its windows cannot be created.
- Settings → **General → Tray menu** or `config.ini`; the change applies the next time the
  menu opens.

## Action modes

Settings → **Voicemeeter → Switch mode** chooses what the keybind does:

- **Toggle (one button on/off)** — the existing behavior: each press flips
  `vm_param` between ON and OFF. This is the default.
- **Switch (two buttons A/B)** — each press swaps `vm_param` (primary) and
  `vm_param_b` (secondary). The primary's current raw state selects the
  other side:
  - primary ON → primary OFF, secondary ON
  - primary OFF → primary ON, secondary OFF

  This also keeps exactly one side ON when both started ON or both started
  OFF. If the two parameters are identical, switch behaves like a normal
  toggle. Both buttons can be on any strip and any button (for example
  `Strip[0].B1` and `Strip[3].B2`).

The overlay dot and tooltip always follow the **primary** parameter; a
`(A/B switch)` suffix appears in the tooltip while switch mode is active.
`invert` remains display-only for the primary. Switching writes raw 0/1
values, so both parameters must be readable; if either one is unknown,
Voicemeeter is not running, or a write fails, the press does nothing and
the existing state is left alone.

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
  (0 OK, -2 no server, -3 unknown parameter) powers the action keybind.
  Toggle mode writes one parameter; switch mode reads both, then writes
  the complementary pair.
- `vm_param` and `vm_param_b` are zero-indexed: UI "Strip 1" = `Strip[0]`.
  Examples: `Strip[0].B2`, `Strip[0].Mute`, `Bus[1].Mute`.
- `invert=true` flips the primary's green/red display, so parameters with
  opposite logic (e.g. `Strip[0].Mute = 1` means muted) still read
  correctly. It does not change which raw value switch mode writes.

## Action keybind / hold

The overlay can drive the configured Voicemeeter button(s) itself, so
Voicemeeter's own macro button is optional. Settings → **Hotkey → Trigger behaviour**:

- **Toggle (press)** — key/button down flips the parameter permanently
  (or swaps the A/B pair in switch mode).
- **Hold (push-to-talk)** — in toggle mode, while held the mic is forced
  ON (green), on release it returns to the state saved on key-down. In
  switch mode this is a momentary A/B swap: press selects the other side,
  release restores both saved raw states.
- **Both** — tap faster than `hold_tap_ms` toggles (or swaps permanently
  in switch mode), holding longer is momentary (release restores the
  saved state).

Details:

- Settings → **Hotkey → Capture key** → press a keyboard key or mouse button
  anywhere (Left/Right/Middle/X1/X2 side buttons). Modifier-only presses
  (Shift/Ctrl/Alt/Win) are ignored, so Ctrl+A binds A. Esc cancels, the
  capture also times out after 15 s, and Esc or any tray click cancels it.
- The event is always **passed through**: the key keeps working normally in
  games/apps AND drives the mic.
- Keyboard auto-repeat is suppressed: one press = one down/up pair, holding
  never spams toggles. Key-up is only used by Hold/Both (ignored in Toggle).
- In toggle mode, Hold writes the value that renders as ON (accounts for
  `invert`), so `invert=true` still gives green while held. In switch mode,
  Hold restores the exact raw A/B states saved on press.
- If Voicemeeter is unreachable (gray state) or either switch parameter is
  unknown, presses are ignored. If you change a parameter manually
  mid-hold, the release restores the saved state and overwrites that
  manual change (expected for simple PTT / momentary switch).
- `hold_tap_ms` (Both only) is configurable via **Settings → Hotkey →
  Hold/tap threshold** (slider + manual value) or in `config.ini`
  (100–2000, default 300).
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
- No sound, no clickable overlay, no settings UI inside the overlay itself
  (a separate `onair-settings.exe` ships alongside it), no separate
  hotkey driver or background service, no autostart, no auto-update, no
  telemetry, no log files. The optional keybind is handled in-process.
- Windows 10/11 64-bit only.

## Layout

```
Cargo.toml         // workspace: root package onair + crates/onair-*
build.rs       // embeds assets/onair.ico + version metadata
assets/
  onair.ico
LICENSE
src/
  main.rs      // entry, single instance, message loop, timers, WndProc, tray IPC
  window.rs    // helpers: lock / topmost / pos / clamp / drag via HTCAPTION
  render.rs    // DIB + SDF rasterizer + UpdateLayeredWindow + tray icon
  vm.rs        // dynamic DLL loading, login, polling, MicState
  tray.rs      // Shell_NotifyIcon, minimal menu model, native menu, tooltip
  glassmenu.rs // custom acrylic tray menu (rendered from tray::build_model)
  config.rs    // re-exports onair-config (load / save / defaults / clamps)
  hotkey.rs    // WH_KEYBOARD_LL + WH_MOUSE_LL mic toggle bind + capture
crates/
  onair-config/ // shared config parsing / clamps / atomic update / settings-write
  onair-settings/ // Slint settings window (native frame, DWM title bar,
                 //  light/dark theme, debounced writes, live reload banner)
```

IPC (registered window messages): `OnAir.ReloadConfig` (settings → overlay,
reload config), `OnAir.BeginCapture` (settings → overlay, start key capture);
the overlay window class is `OnAirOverlay`.
