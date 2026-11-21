# Battery Overlay

Windows tray/overlay app (Rust) that shows battery levels of wireless devices (Bluetooth, Xbox, Logitech/Razer/SteelSeries/Corsair/Lamzu dongles; ATK best-effort) and alerts when low — must stay visible over fullscreen/borderless games.

## Hard constraints
- Performance first: idle RAM < 15 MB, 0% CPU between polls, redraw only on change. Main process = raw Win32 + Direct2D/DirectComposition via `windows` crate; heavy UI (egui settings) runs only as a separate `--settings` process.
- Overlay = transparent, click-through, topmost, no-activate window. NEVER inject/hook DirectX (anti-cheat risk).
- Never use Windows toast notifications for alerts (suppressed during games) — draw alerts in our overlay.
- Open vendor HID devices non-exclusively (coexist with G Hub/Synapse/iCUE).
- Protocol references (Solaar, OpenRazer, HeadsetControl, ckb-next, OpenLinkHub) are mostly GPL: implement from protocol knowledge, don't copy code.
- Two user-selectable overlay modes: Alerts-only, or HUD + alerts (hotkey toggles HUD).
- Settings window (`src/settings.rs`) = `--settings` egui/eframe process, opened from the tray. It saves the config (throttled) and posts the registered `BatteryOverlay.ConfigChanged` message to the main window (class `BatteryOverlay.Main`), which reloads only mode + HUD settings.

## Build & test
- Developed on macOS, runs on Windows only.
- Build: `cargo xwin build --release --target x86_64-pc-windows-msvc`
- Tests: `cargo test` — keep alert logic, config, and HID protocol parsers platform-independent so they test on macOS.
- Config lives at `%APPDATA%\BatteryOverlay\config.toml`.
- Can't run the app here: verify on Windows with `battery_overlay.exe --probe [--all] > out.txt` (what providers see / every raw device property plus unsupported vendors' vendor-defined HID collections) or `--demo` (placeholder devices, overlay only).

## Providers (`src/providers/`)
- One module per source; `poll_all()` merges them. Pure parsing/merging at the top of each module, Win32 queries in a `#[cfg(windows)] mod win`.
- Bluetooth reads Windows' undocumented device-node properties via CfgMgr32 (battery `{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2`, connected `{83DA6326-97A6-4088-9453-A1923F573B29} 15`) — no GATT/WinRT. Levels stay after disconnect, so disconnected devices are hidden. No charging info is exposed for Bluetooth; charging comes from vendor protocols.
- Battery changes raise no OS event: levels refresh only on the poll timer; `WM_DEVICECHANGE` triggers re-polls at 250 ms and 3 s (Windows publishes the level late).
- `poll_all()` runs on a short-lived worker thread (vendor HID replies can take up to 500 ms per request) and posts `WM_POLLED` back; at most one poll runs at a time.
- Vendor providers (Logitech HID++, Razer, SteelSeries, Corsair, Lamzu) share `hid.rs`: USB only (Bluetooth HID is left to the Bluetooth provider), filtered by vendor ID before opening, opened shared. Protocol logic is written against the `Channel`/`FeatureChannel` traits and tested with `MockChannel`; replies are matched by content because every open handle sees every input report (including G Hub's/iCUE's traffic).
- Xbox uses XInput (4 coarse levels mapped to 5/20/60/100 %); Bluetooth-paired Xbox controllers are de-duplicated in favour of the Bluetooth reading.
- SteelSeries and Corsair byte layouts are best-effort and unverified on hardware. Lamzu was decoded from a USB capture of the Maya X web driver and verified on it (level, charging on the cable, hidden when switched off). ATK has no implementation yet.
