// Release builds use the GUI subsystem so no console window appears.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// Platform-independent modules are only fully used by the Windows build.
#![cfg_attr(not(windows), allow(dead_code))]

mod config;
mod hotkey;
mod model;
mod overlay;
mod probe;
mod providers;
mod settings;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod tray;

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if has("--probe") {
        probe::run(has("--all"));
        return;
    }
    if let Err(e) = app::run(has("--demo")) {
        eprintln!("battery_overlay: {e}");
        std::process::exit(1);
    }
}
