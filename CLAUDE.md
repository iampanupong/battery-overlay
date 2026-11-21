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
- Can't run the app here: verify on Windows with `battery_overlay.exe --probe [--all] > out.txt` (what providers see / every raw device property) or `--demo` (placeholder devices, overlay only).

## Providers (`src/providers/`)
- One module per source; `poll_all()` merges them. Pure parsing/merging at the top of each module, Win32 queries in a `#[cfg(windows)] mod win`.
- Bluetooth reads Windows' undocumented device-node properties via CfgMgr32 (battery `{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2`, connected `{83DA6326-97A6-4088-9453-A1923F573B29} 15`) — no GATT/WinRT. Levels stay after disconnect, so disconnected devices are hidden. No charging info is exposed for Bluetooth; charging comes from vendor protocols.
- Battery changes raise no OS event: levels refresh only on the poll timer; `WM_DEVICECHANGE` triggers re-polls at 250 ms and 3 s (Windows publishes the level late).
