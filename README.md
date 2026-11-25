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

Sizes: `onair.exe` **370,176 bytes** (~362 KB). The settings exe cannot be
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
- Visual ▸ (Dot / Ring / Bar / Mini / Wave)
- Wave ▸ (Sensitivity −35/−40/−45/−50/−55 dB, 3/5/7 bars, 20/30/45 FPS)
- Lock position (checkmark)
- Always on top (checkmark, re-asserted ~every 2 s)
- Show overlay (checkmark)
- Mute indicator (checkmark)
- Quit

The **Visual** and **Wave** submenus change the running overlay
immediately; the values are saved to `config.ini`.

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

- **General** – size, opacity, lock position, always on top, show/hide the
  overlay, tray menu style (Glass acrylic / Native Win32).
- **Appearance** – visual style (Dot/Ring/Bar/Mini/Wave), background opacity,
  colours (on/off/unknown/background) with hex validation or a click-to-open
  HSV colour wheel (hue/saturation disc + value slider, Reset to defaults),
  and a live on-screen preview with a pulse animation.
- **Appearance → Mute indicator** – toggle the mute sign and pick its colour.
- **Voicemeeter** – toggle vs two-button switch mode, primary/secondary
  parameter with strip/button dropdowns or a raw "Custom" value, invert,
  poll interval.
- **Hotkey** – capture the global toggle key (the overlay installs the
  low-level hook; Settings just asks it to and polls `config.ini`),
  trigger mode and hold/tap threshold.
- **Macros** – any number of extra global keys that run Voicemeeter requests,
  each with its own Press/Hold/Both behaviour and optional release script.
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
overlay_visible=true
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
speak_threshold_db=-45
speak_release_ms=300
anim_fps=30
wave_bars=5
level_type=0
level_channel=auto
mute_indicator=true
color_mute=E74C3C
menu_style=glass
color_on=2ECC71
color_off=E74C3C
color_unknown=808080
color_bg=141414
retrigger_ms=40
debug_log=false
macro=Ctrl+Alt+F13|press|Mic mute|Strip[0].B2=!|
macro=F14|hold|Gain up|Strip[0].Gain+=1.5|Strip[0].Gain-=1.5
```

Clamps: `size` 16–512, `opacity` 20–100, `poll_ms` 30–1000,
`hold_tap_ms` 100–2000, `bg_opacity` 0–100, `speak_threshold_db` −70…−20,
`speak_release_ms` 100–1500, `anim_fps` 10–60, `wave_bars` 3–9,
`level_type` 0–3, `retrigger_ms` 0–500. Colors are 6-digit hex without `#`.
`trigger_mode` accepts `toggle` / `hold` / `both` (aliases: `press`,
`ptt`, `toggle+hold`); unknown falls back to `toggle`.
`vm_mode` accepts `toggle` / `switch` (aliases: `swap`, `a/b`, `alternate`,
`two buttons`); unknown falls back to `toggle`.
`visual_style` accepts `dot` / `ring` / `bar` / `mini` / `wave`
(aliases: `strip`/`pill` → bar, `circle`/`outline` → ring,
`minimal`/`nobg` → mini, `voice`/`speaking` → wave); unknown falls back
to `dot`.
`menu_style` accepts `glass` / `native` (aliases: `acrylic`/`blur`/`modern`
→ glass, `classic`/`win32`/`system` → native); unknown falls back to `glass`.
`overlay_visible` (`true`/`false`, alias `show_overlay`) hides the window
without stopping hotkeys. `macro=` rows are repeatable and documented in
[Macro buttons](#macro-buttons). `retrigger_ms` (0–500, default 40) drops a
second press of the same trigger inside that window, which filters mouse
switch chatter and duplicated software-injected clicks; `0` disables it.
`debug_log` (`true`/`false`, default `false`) is documented in
[Troubleshooting](#troubleshooting-debug-log).

## Visual styles (all small, same square hitbox)

- **Dot** — rounded square + filled circle (default, most visible).
- **Ring** — rounded square + hollow colored ring, dark center
  (less glare at night, still easy to read).
- **Bar** — rounded square + thin colored strip at the bottom
  (smallest color area, good for corner placement).
- **Mini** — no background, just a small dot (~54% size)
  (minimal footprint; use 32–48 px, 80–100% opacity).
- **Wave** — rounded square + 3–9 vertical capsule bars driven by the
  Voicemeeter input level. Bars animate only while the microphone button is
  **ON** and the level is above the sensitivity threshold; quiet, muted or
  unknown states show flat minimum-height dots. No audio capture: the level
  comes from `VBVMR_GetLevel` only.

Settings → **Appearance → Visual** switches immediately and saves `config.ini`.
Tray icon always stays Dot for recognizability.

## Wave style

The wave reads the level of the strip in `vm_param` (`Strip[n].…`):

- `level_type` selects the tap: `0` pre-fader (default), `1` post-fader,
  `2` post-mute, `3` output level. Post-mute (`2`) also drops to silence
  when the strip is muted by Voicemeeter itself.
- `level_channel=auto` maps the strip automatically for the detected
  edition (Standard 2 / Banana 3 / Potato 5 hardware strips; virtual strips
  use 8 channels each). A number overrides the left channel and uses the
  next channel for the right; the louder one wins.
- A `vm_param` that is not a `Strip[n]` parameter (bus, macro button, custom
  name) has no level mapping; it shows the static minimum frame unless
  `level_channel` is set manually.
- `speak_threshold_db` (−70…−20, default −45) is silence; −10 dB is treated
  as full scale. `speak_release_ms` (100–1500, default 300) is the hangover
  before the animation stops and the bars settle at their minimum.
- `anim_fps` (10–60, default 30) and `wave_bars` (3–9, default 5) control
  the timer only while speaking; idle polling never starts it.
- Tray → **Wave** has quick presets (sensitivity, bars, FPS). The full
  range is available in `config.ini`.

The renderer reuses one pixel buffer and one DIB per size, so an active
30 FPS 48 px overlay stays well below ~1.5 % of one core and allocates
nothing per frame.

## Mute indicator

When Voicemeeter mutes the strip used by `vm_param` (`Strip[n].Mute`), the
overlay is replaced by a prohibition sign (ring + diagonal slash) so a muted
microphone is obvious even while the on-air button is still ON. Unmuting
restores the configured style and, for Wave, resumes the animation.

- `mute_indicator` (`true`/`false`, default `true`, aliases `show_mute`,
  `mute_badge`) enables the replacement. Tray → **Mute indicator** and
  Settings → **Appearance** toggle it.
- `color_mute` (default `E74C3C`, alias `mute_color`) is the ring/slash
  colour; the background keeps the normal `color_bg` and `bg_opacity`.
- The mute state is read on every poll (`poll_ms`), so it reacts within one
  tick. A `vm_param` that is not a `Strip[n]` parameter has no mute state and
  keeps the normal overlay.
- The tray icon stays Dot; the tooltip appends `(muted)` while muted.

## Background transparency (`bg_opacity`)

- Only the rounded-square background fades; the colored
  dot/ring/strip stays fully opaque for readability.
- Global **Opacity** still applies on top of everything
  (e.g. `opacity=50` + `bg_opacity=50` = very faint bg).
- `bg_opacity=0` = no background at all (floating dot/ring/strip).
  While unlocked the whole `size`×`size` square stays grabbable: an invisible
  ~2% alpha floor keeps the layered window hit-testable, so a click anywhere on
  the square drags it (not only on the thin white outline). When locked the
  square is fully click-through.
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
  (0 OK, -2 no server, -3 unknown parameter), plus the optional
  `VBVMR_GetLevel(type, channel, &mut value)` and
  `VBVMR_GetVoicemeeterType(&mut type)` used by the wave style (1 Standard,
  2 Banana, 3 Potato). Old DLLs without the level exports still show
  ON/OFF normally. Verified against the
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
  anywhere (Left/Right/Middle/X1/X2 side buttons). Modifier combos work:
  hold Ctrl/Alt/Shift/Win while pressing the key, and the captured bind is
  e.g. `Ctrl+Alt+F13`. Modifier-only presses are not bindable, so Ctrl+A
  binds Ctrl+A (not just A). Esc cancels, the capture also times out after
  15 s, and Esc or any tray click cancels it.
- Modifier matching is **exact**: a bare `F13` macro does not fire while
  Ctrl is held, so the same key can host both `F13` and `Ctrl+F13`.
- The event is always **passed through**: the key keeps working normally in
  games/apps AND drives the mic.
- Keyboard auto-repeat is suppressed: one press = one down/up pair, holding
  never spams toggles. Key-up is only used by Hold/Both (ignored in Toggle).
- A second press of the same trigger within `retrigger_ms` (default 40 ms) is
  ignored. Worn mouse switches and vendor software that emit two down/up
  pairs for one physical click would otherwise toggle twice (on, then off).
  Set `retrigger_ms=0` to disable the guard.
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
- Numpad keys are captured by their physical key as `Num0`–`Num9`, `Num+`,
  `Num-`, `Num*`, `Num/`, `Num.` and keep working with NumLock on or off.
  A dedicated Insert/Delete/Home/arrow key is kept separate from the numpad
  key that shares its position.
- Technique: `WH_KEYBOARD_LL` + `WH_MOUSE_LL` on a dedicated hotkey thread
  with its own minimal message loop (Discord-style push-to-talk) — no
  injection, no game memory access, no admin needed. Windows delivers a
  low-level hook *through the installing thread*, so a slow Voicemeeter poll,
  disk write or menu on the UI thread used to swallow presses; separating them
  means a busy overlay never drops a trigger. Modifiers are read live from the
  keyboard state, so a missed key-up (lock screen, UAC, RDP) cannot leave
  `Ctrl` stuck and make the hotkey look dead until a restart.
- Hooks are installed only while a bind exists or capture is active, and are
  re-checked every 30 s: if Windows silently removed one (no API reports it),
  it is reinstalled, and a failed install is retried. If installation keeps
  failing, the feature stays inactive and the debug log says so.

## Macro buttons

Settings → **Macros** turns OnAir into a lightweight Voicemeeter MacroButtons
replacement. Add any number of macros, capture a global key for each, and give
it a Voicemeeter action. The overlay installs the same low-level hooks as the
main hotkey; there is no practical limit on the number of macros.

For the common case you do not need to know Voicemeeter scripting at all:
**Quick setup** has **Strip**, **Button** (A1–A5, B1–B3, Mute) and **Action**
(Toggle / Turn ON / Turn OFF) dropdowns. Picking them writes the press script
for you, e.g. `Strip[0].B2=!`. The script fields stay available under
**Advanced** for multiple requests or parameters the picker does not cover.

Each macro row is stored as one `macro=` line:

```
macro=<trigger>|<mode>|<label>|<press>|<release>
```

- **trigger** – any key or mouse button, optionally with modifiers, e.g.
  `F13`, `Ctrl+F13`, `Shift+Mouse X1`.
- **mode** – `press` runs the press script once; `hold` runs it on key-down
  and restores on release; `both` keeps a short tap and restores after a long
  hold (`hold_tap_ms`, same threshold as the main hotkey).
- **label** – shown in the macro dropdown.
- **press / release** – Voicemeeter requests separated by `;` or newlines:
  - `Strip[0].B2=1` sets a value.
  - `Strip[0].B2=!` toggles a 0/1 parameter.
  - `Strip[0].Gain+=1.5` / `Strip[0].Gain-=0.5` adjust a numeric value.
- With an empty release script, Hold/Both restore the parameter values read on
  key-down, so a PTT-style gain or mute works without writing a release line.

Two macros cannot share the same trigger (including the same base key with a
different modifier set). Capturing is handled by the overlay; Settings asks it
to listen and polls the `macro_capture` hand-off key. Macro rows are validated
before saving and the overlay reloads immediately.

The overlay's **Show overlay** switch (Settings → General, or the tray menu)
hides the indicator without stopping OnAir. Hotkeys, macros and the tray icon
keep working while it is hidden.

## Troubleshooting (debug log)

OnAir is silent by default. To capture what actually happens, add
`debug_log=true` to `config.ini` (or start it once with the `ONAIR_DEBUG=1`
environment variable) and reproduce the problem. A log appears next to
`config.ini`:

- `onair-debug.log` — current run.
- `onair-debug.old.log` — the previous run, kept for comparison.

Every line is `[+ms] ...` relative to start. The log is capped at 4 MiB per
run, then stops writing.

What is recorded:

- **Trigger events** for every key/button that drives a hotkey or macro:
  virtual key, scancode, `injected` flag, the OS event time and the modifier
  state. Two `down` lines a few ms apart for one physical press mean chatter
  (hardware) or a duplicated injected click (vendor software), not an OnAir
  bug. A `down` with no preceding physical press and `injected=true` points at
  another program sending the button. Suppressed presses say
  `suppressed: retrigger guard`. A press with `mods=0x1` and no following
  `key down` means a modifier really was held (triggers match modifiers
  exactly), not that the hotkey was ignored.
- **Hotkey plumbing**: `hotkey thread spawned`, `hotkey hooks installed kb=...
  ms=...`, `hotkey hook had been removed by windows, reinstalled` and
  `released a stale press (key-up was missed)`. Hooks run on their own thread,
  so a slow poll cannot swallow a press; these lines prove the hooks are alive
  and that lost key-ups self-heal.
- **Hold decisions**: `hold up ... dur=NNms threshold=NNms tap=true/false`.
  With `trigger_mode=both`, a press released later than `hold_tap_ms` is
  momentary and reverts — that is the mode working as designed, not a
  spontaneous toggle.
- **Voicemeeter writes**: the value read before, the value written and the
  Remote API return code (`vm switch ...`, `vm set ...`).
- **Window events**: `drag start (unlocked)`, `drag end at X,Y`,
  `lock=... exstyle 0x...` and `rejected foreign geometry ...` show whether a
  move came from the user or from another process. A drag that never logs
  `drag start` means the click never reached the overlay.
- **Slow calls** (`SLOW ...`): any Remote API, tray or Shell call that blocked
  the UI thread for 25 ms or more.
- **Watchdog**: `WATCHDOG ui thread stalled for Nms, stage=poll|action|menu`
  is written *while* the UI thread is blocked, so a "Not Responding" episode
  names the operation that caused it.

The menu also has a fallback dismissal: if Windows denies it foreground
rights, a click outside (or Esc) still closes it instead of leaving the app
in a modal loop.

## Safety / non-goals

- Separate top-most window only, plus low-level input hooks for the optional
  toggle keybind. No hooks into the game, no DLL injection,
  no game-memory reading.
- No sound, no clickable overlay, no settings UI inside the overlay itself
  (a separate `onair-settings.exe` ships alongside it), no separate
  hotkey driver or background service, no autostart, no auto-update, no
  telemetry. No log files unless `debug_log=true` is set explicitly; the
  optional keybind is handled in-process.
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
  wave.rs      // pure wave level math, smoothing and speaking/quiet state
  vm.rs        // dynamic DLL loading, login, polling, MicState
  tray.rs      // Shell_NotifyIcon, minimal menu model, native menu, tooltip
  glassmenu.rs // custom acrylic tray menu (rendered from tray::build_model)
  config.rs    // re-exports onair-config (load / save / defaults / clamps)
  hotkey.rs    // WH_KEYBOARD_LL + WH_MOUSE_LL on their own thread: toggle bind + capture
crates/
  onair-config/ // shared config parsing / clamps / atomic update / settings-write
  onair-settings/ // Slint settings window (native frame, DWM title bar,
                 //  light/dark theme, debounced writes, live reload banner)
```

IPC (registered window messages): `OnAir.ReloadConfig` (settings → overlay,
reload config), `OnAir.BeginCapture` (settings → overlay, start key capture);
the overlay window class is `OnAirOverlay`.
