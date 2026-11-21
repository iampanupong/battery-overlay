// Release builds use the GUI subsystem so no console window appears.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// Platform-independent modules are only fully used by the Windows build.
#![cfg_attr(not(windows), allow(dead_code))]

mod config;
mod hotkey;
mod model;
mod overlay;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod tray;

#[cfg(windows)]
fn main() {
    if let Err(e) = app::run() {
        eprintln!("battery_overlay: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("battery_overlay runs on Windows only; use `cargo test` on this platform.");
    std::process::exit(1);
}
