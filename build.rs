// The settings window (eframe/winit/glow) links these DLLs, but the always-running
// overlay never calls them. Delay-loading keeps them out of the overlay process;
// they load on first use, i.e. only in the `--settings` process.
const SETTINGS_ONLY_DLLS: [&str; 4] = ["opengl32.dll", "imm32.dll", "uxtheme.dll", "dwmapi.dll"];

fn main() {
    let target = |key: &str| std::env::var(key).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") == "windows" && target("CARGO_CFG_TARGET_ENV") == "msvc" {
        for dll in SETTINGS_ONLY_DLLS {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
        }
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
}
