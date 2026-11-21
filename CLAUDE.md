# Battery Overlay

Windows tray/overlay app (Rust) that shows battery levels of wireless devices (Bluetooth, Xbox, Logitech/Razer/SteelSeries/Corsair dongles; ATK/Lamzu best-effort) and alerts when low — must stay visible over fullscreen/borderless games.

## Hard constraints
- Performance first: idle RAM < 15 MB, 0% CPU between polls, redraw only on change. Main process = raw Win32 + Direct2D/DirectComposition via `windows` crate; heavy UI (egui settings) runs only as a separate `--settings` process.
- Overlay = transparent, click-through, topmost, no-activate window. NEVER inject/hook DirectX (anti-cheat risk).
- Never use Windows toast notifications for alerts (suppressed during games) — draw alerts in our overlay.
- Open vendor HID devices non-exclusively (coexist with G Hub/Synapse/iCUE).
- Protocol references (Solaar, OpenRazer, HeadsetControl, ckb-next, OpenLinkHub) are mostly GPL: implement from protocol knowledge, don't copy code.
- Two user-selectable overlay modes: Alerts-only, or HUD + alerts (hotkey toggles HUD).

## Build & test
- Developed on macOS, runs on Windows only.
- Build: `cargo xwin build --release --target x86_64-pc-windows-msvc`
- Tests: `cargo test` — keep alert logic, config, and HID protocol parsers platform-independent so they test on macOS.
- Config lives at `%APPDATA%\BatteryOverlay\config.toml`.
