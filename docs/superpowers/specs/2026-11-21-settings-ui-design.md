# Settings UI (prototype) — design

## Intent
Let the user change the HUD's position, opacity, size and the overlay mode from a window instead of hand-editing `config.toml`, and see the HUD update live while adjusting.

## Goals
- Settings window with: corner picker (2×2), opacity slider (20–100 %), size slider (50–300 %, maps to `hud.scale`), mode radio (Alerts only / HUD + alerts).
- Changes apply live to the running overlay.
- Opened from a new tray menu item **Settings…**.

## Non-goals (prototype)
- Free X/Y placement, dragging, monitor selection, margin slider.
- Editing hotkey, thresholds or poll interval.
- Live pickup of hand-edits to `config.toml`.

## Anti-goals
- Anything that raises the main process's idle RAM/CPU (no file watcher, no extra threads, no egui code paths touched in the overlay process).

## Constraints
- CLAUDE.md: heavy UI runs only as a separate `--settings` process (egui).
- Config values stay within the existing `Config::sanitized()` ranges.

## Approaches
- **A. Separate egui process that saves and pings the overlay (chosen).** Follows the project design; tiny change in the main app; cost is a bigger exe and longer compile.
- B. Same window, overlay watches the config file. Rejected: adds a watcher to the always-running process for a benefit (hand-edit pickup) not asked for.
- C. Native Win32 dialog in the main process. Rejected: violates the separate-process constraint and is a lot of hand-built UI.

## Design
- **Dependency:** `eframe` (egui) as a Windows-only dependency.
- **`--settings` process** (`src/settings.rs`, Windows-only): loads config, shows the four controls, and on every change saves the config with `config::save` and posts a registered window message `BatteryOverlay.ConfigChanged` to the main window (found by its class `BatteryOverlay.Main`). If the main app isn't running it just saves.
- **Main app:** handles `BatteryOverlay.ConfigChanged` by reloading the config and redrawing the HUD. If the mode changed, the HUD's visibility resets to match the mode (same as the tray mode items do today). A tray item **Settings…** spawns `current_exe() --settings`.
- **Single settings window:** a second **Settings…** while one is open doesn't open another (named mutex; bringing the existing one to front is nice-to-have).
- **Testability:** any pure logic (e.g., mapping UI values to config) stays outside `#[cfg(windows)]` and gets unit tests.

## Left to the builder
- Throttling saves during slider drags (e.g., ≤ ~10 saves/s) and tolerating a failed rename while the main app is reading — builder's call.
- Window size, layout and labels.
- Whether the main app reloads hotkey/poll interval too on the message (not required).

## Revision 2026-11-22 (after first Windows test)
Feedback: sliders weren't smooth; the window looked bad.
- **Smooth preview:** while a slider is dragged, the settings window posts `BatteryOverlay.Preview` with mode/corner/opacity %/size % packed into `wparam` — no file I/O. The main window applies only the newest queued preview. The file is saved when a slider is released (or on a click / keyboard step). The overlay now draws before moving/resizing its window, so size changes don't flash a stretched frame.
- **New look:** dark palette from the HUD (`#121418` background, `#4DD973` accent), Segoe UI, section cards, a segmented mode control with a one-line hint, a clickable mini-screen corner picker, full-width sliders with the value beside the label, "Reset to defaults" and a save status. Typing slider values was dropped.
- `--demo` keeps showing placeholder devices by design; real devices appear without `--demo`.
