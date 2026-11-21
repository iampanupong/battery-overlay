# Battery Overlay

A lightweight Windows tray app that shows the battery levels of your wireless devices in a click-through overlay and warns you when they run low, even while a game is running fullscreen.

**Status:** Phase 1 (skeleton). The tray icon, settings file, hotkey and HUD overlay work, but the HUD shows placeholder devices.

## Planned device support
- Bluetooth (BLE and Classic) devices
- Xbox wireless controllers
- Vendor dongles: Logitech, Razer, SteelSeries, Corsair, Lamzu (ATK best-effort)

## Build
Runs on Windows 10/11 only. You can cross-compile it from macOS or Linux with [cargo-xwin](https://github.com/rust-cross/cargo-xwin):

```bash
cargo xwin build --release --target x86_64-pc-windows-msvc
```

On Windows, use `cargo build --release`.

## Test
```bash
cargo test
```

## Usage
- Right-click the tray icon to switch between **Alerts only** and **HUD + alerts** modes, or to quit.
- Press **Ctrl+Shift+B** to show or hide the HUD.
- Settings are stored in `%APPDATA%\BatteryOverlay\config.toml`.
