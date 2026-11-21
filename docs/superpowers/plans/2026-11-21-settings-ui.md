# Settings UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `--settings` window (egui) to pick HUD corner, opacity, size and overlay mode, applied live to the running overlay.

**Architecture:** New `src/settings.rs`: pure, testable pieces at the top (save throttle, shared names), `#[cfg(windows)] mod win` with the eframe window. The window saves `config.toml` and posts a registered `BatteryOverlay.ConfigChanged` message to the main window; the main app reloads the config and redraws. A tray item **Settings…** spawns the process.

**Tech Stack:** Rust, `eframe` 0.37 (Windows-only dependency), `windows` 0.58, `cargo test` on macOS, `cargo xwin`.

**Spec:** `docs/superpowers/specs/2026-11-21-settings-ui-design.md`

**Shell:** cargo isn't on PATH — prefix commands with `export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$HOME/.cargo/bin:$PATH"`.

**Branch:** work on `feature/settings-ui` off `dev`; commit spec + plan as the first commit.

## Global Constraints
- Main process must not touch egui/eframe code; idle RAM < 15 MB, 0 % CPU between polls. No file watcher, no new threads in the main process.
- `eframe` goes under `[target.'cfg(windows)'.dependencies]` only, so `cargo test` on macOS doesn't build it. Use `default-features = false` with `glow` + `default_fonts` (no wgpu, no persistence).
- Values written must stay within `Config::sanitized()` ranges: opacity 0.2–1.0, scale 0.5–3.0. Sliders show percent: opacity 20–100 %, size 50–300 %.
- Position = four corners only. No margin, hotkey, threshold or poll-interval controls.
- Settings process saves the whole loaded `Config` (other fields such as hotkey/thresholds are preserved as loaded).
- Names: main window class `BatteryOverlay.Main` (already in `app.rs`), message `BatteryOverlay.ConfigChanged`, settings mutex `Local\BatteryOverlay.Settings`.
- Saves throttled to at most one per 100 ms, with a trailing save so the final slider value always lands.

## Review Focus
- Slider released between throttle windows: the last value must still be saved and applied (trailing save) — test in Task 1.
- HUD hidden via hotkey in HUD + alerts mode, then only opacity changed: HUD must stay hidden; a mode change resets visibility to the mode — test in Task 1.
- A failed save (rename racing the main app's read) must be retried on a later frame, not dropped — test in Task 1.
- Settings opened while the overlay isn't running: saves, no panic, no error dialog (FindWindow returns null → skip posting) — manual check in Task 2.
- Clicking **Settings…** twice must not open a second window — manual check in Task 3.

---

### Task 1: Pure settings logic

**Files:**
- Create: `src/settings.rs` (pure part only for now)
- Modify: `src/main.rs` (`mod settings;` — platform-independent, like `config`)
- Modify: `src/config.rs` (add `hud_visible_after_reload`)

**Interfaces:**
- Produces (in `settings.rs`):
  - `pub const MAIN_WINDOW_CLASS: &str = "BatteryOverlay.Main";`
  - `pub const CONFIG_CHANGED_MSG: &str = "BatteryOverlay.ConfigChanged";`
  - `pub const SETTINGS_MUTEX: &str = "Local\\BatteryOverlay.Settings";`
  - `pub const SAVE_INTERVAL: Duration = Duration::from_millis(100);`
  - `pub struct SaveThrottle` (`Default`) with `mark_dirty(&mut self)`, `due(&self, now: Instant) -> bool`, `saved(&mut self, now: Instant)`, `wait(&self, now: Instant) -> Option<Duration>` (time until a pending save is due; `None` when nothing is pending).
- Produces (in `config.rs`): `pub fn hud_visible_after_reload(old: OverlayMode, new: OverlayMode, visible: bool) -> bool` — unchanged mode keeps `visible`; changed mode returns `new == HudAndAlerts`.

- [ ] **Step 1: Write the failing tests** in `settings.rs` `mod tests`, using a fixed `let t0 = Instant::now();`:
  - `first_change_saves_immediately`: dirty → `due(t0)`; after `saved(t0)` → `!due(t0)`.
  - `clean_throttle_is_never_due`: `!due(t0)`, `wait(t0) == None`.
  - `changes_within_interval_wait_then_save`: `saved(t0)`, `mark_dirty()` → `!due(t0 + 40ms)`, `wait(t0 + 40ms) == Some(60ms)`, `due(t0 + 100ms)` (trailing save).
  - `failed_save_stays_due`: dirty, `due(t0)`, no `saved` → still `due(t0 + 1ms)`.
  - In `config.rs` tests, `reload_keeps_hotkey_visibility_unless_mode_changes`: `(HudAndAlerts, HudAndAlerts, false) → false`; `(AlertsOnly, HudAndAlerts, false) → true`; `(HudAndAlerts, AlertsOnly, true) → false`; `(AlertsOnly, AlertsOnly, true) → true`.

- [ ] **Step 2: Run** `cargo test` — expected: compile failure (`SaveThrottle`, `hud_visible_after_reload` not defined).

- [ ] **Step 3: Implement** `SaveThrottle` (fields `last_save: Option<Instant>`, `dirty: bool`; due = dirty && (no last save || elapsed ≥ `SAVE_INTERVAL`)) and `hud_visible_after_reload`. Module doc: "Settings window (`--settings`), a separate process so the overlay never loads egui."

- [ ] **Step 4: Run** `cargo test` — expected: all tests pass.

- [ ] **Step 5: Commit** `feat(settings-ui): add save throttle and reload visibility rule`

### Task 2: eframe settings window and `--settings` flag

**Files:**
- Modify: `Cargo.toml` (Windows-only `eframe = { version = "0.37", default-features = false, features = ["default_fonts", "glow"] }`)
- Modify: `src/settings.rs` (`#[cfg(windows)] mod win` + `#[cfg(windows)] pub use win::run;`)
- Modify: `src/main.rs` (`if has("--settings") { settings::run(); return; }` before the overlay starts)

**Interfaces:**
- Consumes: Task 1 constants and `SaveThrottle`; `config::{config_path, load, save, Config, Corner, OverlayMode}`.
- Produces: `pub fn run()` (Windows) — blocks until the window closes; errors go to `eprintln!`.

- [ ] **Step 1: Implement `run()`**:
  - Acquire `SETTINGS_MUTEX` with `CreateMutexW`; if it already exists, bring the existing window to front (`FindWindowW(None, "Battery Overlay Settings")` + `SetForegroundWindow`) and return.
  - Load config with `config::load(&config_path())`; register the message with `RegisterWindowMessageW(CONFIG_CHANGED_MSG)`.
  - eframe window titled `Battery Overlay Settings`, fixed size ~320×300 logical px, not resizable.
  - Controls, top to bottom: **Mode** radio (`Alerts only` / `HUD + alerts`); **Position** 2×2 grid of selectable buttons (`Top left`, `Top right`, `Bottom left`, `Bottom right`); **Opacity** slider over `hud.opacity` 0.2–1.0 shown as percent; **Size** slider over `hud.scale` 0.5–3.0 shown as percent. Any `changed()` → `throttle.mark_dirty()`.
  - Each frame: if `throttle.due(now)` → `config::save`; on `Ok` call `throttle.saved(now)` and notify; on `Err` `eprintln!` and leave it pending. If `throttle.wait(now)` is `Some(d)` → `ctx.request_repaint_after(d)`.
  - Notify = `FindWindowW(MAIN_WINDOW_CLASS, None)`; if found, `PostMessageW(hwnd, msg, 0, 0)`; if not, do nothing.
  - On close, flush a pending save (eframe `App::on_exit`).
- [ ] **Step 2: Build** `cargo xwin build --release --target x86_64-pc-windows-msvc` — expected: succeeds with no warnings from `settings.rs`. Then `cargo test` — still passes (eframe not built on macOS).
- [ ] **Step 3: Manual check on Windows (user)**: with the app not running, `battery_overlay.exe --settings` opens, changing values rewrites `%APPDATA%\BatteryOverlay\config.toml`, no crash. Running it twice focuses the first window.
- [ ] **Step 4: Commit** `feat(settings-ui): add egui settings window behind --settings`

### Task 3: Main app reload, tray item, docs

**Files:**
- Modify: `src/app.rs` (register `CONFIG_CHANGED_MSG`, handle it, use `settings::MAIN_WINDOW_CLASS` for the class name, handle `CMD_SETTINGS`)
- Modify: `src/tray.rs` (`pub const CMD_SETTINGS: u32 = 5;`, menu item `Settings…` above Quit, separated)
- Modify: `README.md` Usage, `CLAUDE.md` (one line: settings window = `--settings` egui process; saves config and posts `BatteryOverlay.ConfigChanged`; main reloads HUD/mode only)

**Interfaces:**
- Consumes: `settings::{MAIN_WINDOW_CLASS, CONFIG_CHANGED_MSG}`, `config::hud_visible_after_reload`.
- Produces: `App::reload_config(&mut self)`, `fn open_settings()`.

- [ ] **Step 1: Implement**:
  - `CONFIG_CHANGED: Cell<u32>` thread-local next to `TASKBAR_CREATED`, set from `RegisterWindowMessageW`; wndproc arm `m if m != 0 && m == CONFIG_CHANGED.get()` → `with_app(App::reload_config)`.
  - `reload_config`: `let new = config::load(&self.config_path)`; `self.hud_visible = hud_visible_after_reload(self.config.mode, new.mode, self.hud_visible)`; replace only `mode` and `hud` in `self.config` (hotkey and poll interval stay as running — re-registering them is out of scope); `refresh_hud()`.
  - `open_settings`: `std::process::Command::new(std::env::current_exe()?).arg("--settings").spawn()`; log errors. The settings process's own mutex handles the double-click case.
- [ ] **Step 2: Build** `cargo xwin build --release --target x86_64-pc-windows-msvc` and `cargo test` — both succeed.
- [ ] **Step 3: Manual check on Windows (user)** with `battery_overlay.exe --demo`: tray → Settings… opens the window; dragging opacity/size and clicking corners updates the HUD live; switching mode to Alerts only hides the HUD and back shows it; hide HUD with Ctrl+Shift+B, change opacity → HUD stays hidden; Settings… twice → one window.
- [ ] **Step 4: Commit** `feat(settings-ui): open settings from tray and apply changes live`
