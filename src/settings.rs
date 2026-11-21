//! Settings window (`--settings`), a separate process so the overlay never loads egui.

use std::time::{Duration, Instant};

use crate::config::Config;

/// Class of the main app's hidden window, which receives `CONFIG_CHANGED_MSG`.
pub const MAIN_WINDOW_CLASS: &str = "BatteryOverlay.Main";
/// Registered window message posted after the settings window saves the config.
pub const CONFIG_CHANGED_MSG: &str = "BatteryOverlay.ConfigChanged";
pub const SETTINGS_MUTEX: &str = "Local\\BatteryOverlay.Settings";
/// Minimum gap between saves while a slider is being dragged.
pub const SAVE_INTERVAL: Duration = Duration::from_millis(100);

/// Rate-limits config saves; a change made too soon after the last save stays
/// pending until the interval has passed, so the final value always lands.
#[derive(Default)]
pub struct SaveThrottle {
    last_save: Option<Instant>,
    dirty: bool,
}

impl SaveThrottle {
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn due(&self, now: Instant) -> bool {
        self.dirty && self.wait(now) == Some(Duration::ZERO)
    }

    /// Call after a successful save; a failed one stays pending and is retried.
    pub fn saved(&mut self, now: Instant) {
        self.last_save = Some(now);
        self.dirty = false;
    }

    /// Time until the pending save is due, or `None` when nothing is pending.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        if !self.dirty {
            return None;
        }
        let elapsed = self.last_save.map_or(SAVE_INTERVAL, |t| now.saturating_duration_since(t));
        Some(SAVE_INTERVAL.saturating_sub(elapsed))
    }
}

/// Shows a 0.0–1.0+ fraction (opacity, scale) as a whole percent.
pub fn percent_text(fraction: f64) -> String {
    format!("{:.0}%", fraction * 100.0)
}

/// Parses what the user typed into a slider's value box, with or without "%".
pub fn parse_percent(s: &str) -> Option<f64> {
    let p: f64 = s.trim().trim_end_matches('%').trim().parse().ok()?;
    Some(p / 100.0)
}

/// Applies the fields edited in the settings window (`before` → `edited`)
/// onto the config on disk, so a mode switched from the tray meanwhile, or
/// any other field, isn't overwritten with the window's stale copy.
pub fn apply_edits(mut disk: Config, before: &Config, edited: &Config) -> Config {
    if edited.mode != before.mode {
        disk.mode = edited.mode;
    }
    let (hud, old) = (&mut disk.hud, &before.hud);
    if edited.hud.corner != old.corner {
        hud.corner = edited.hud.corner;
    }
    if edited.hud.opacity != old.opacity {
        hud.opacity = edited.hud.opacity;
    }
    if edited.hud.scale != old.scale {
        hud.scale = edited.hud.scale;
    }
    disk
}

#[cfg(windows)]
pub use win::run;

#[cfg(windows)]
mod win {
    use std::ops::RangeInclusive;
    use std::path::PathBuf;
    use std::time::Instant;

    use eframe::egui;
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, LPARAM, WPARAM};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, PostMessageW, RegisterWindowMessageW, SetForegroundWindow,
    };

    use super::{
        apply_edits, parse_percent, percent_text, SaveThrottle, CONFIG_CHANGED_MSG, MAIN_WINDOW_CLASS, SAVE_INTERVAL,
        SETTINGS_MUTEX,
    };
    use crate::config::{self, Config, Corner, OverlayMode};

    const TITLE: &str = "Battery Overlay Settings";

    /// Shows the settings window until it is closed. If one is already open,
    /// brings it to the front instead.
    pub fn run() {
        // Held until the process exits; the OS releases it.
        let _instance_lock = unsafe { CreateMutexW(None, true, &HSTRING::from(SETTINGS_MUTEX)) };
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            if let Ok(hwnd) = unsafe { FindWindowW(PCWSTR::null(), &HSTRING::from(TITLE)) } {
                let _ = unsafe { SetForegroundWindow(hwnd) };
            }
            return;
        }

        let path = config::config_path();
        let loaded = config::load(&path);
        let app = Settings {
            config: loaded.clone(),
            on_disk: loaded,
            focused: true,
            path,
            throttle: SaveThrottle::default(),
            changed_msg: unsafe { RegisterWindowMessageW(&HSTRING::from(CONFIG_CHANGED_MSG)) },
        };
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title(TITLE)
                .with_inner_size([320.0, 300.0])
                .with_resizable(false)
                .with_maximize_button(false),
            ..Default::default()
        };
        if let Err(e) = eframe::run_native(TITLE, options, Box::new(|_| Ok(Box::new(app)))) {
            eprintln!("settings window failed: {e}");
        }
    }

    struct Settings {
        /// What the controls show and edit.
        config: Config,
        /// The config as last loaded or saved; `config` minus this = the user's edits.
        on_disk: Config,
        focused: bool,
        path: PathBuf,
        throttle: SaveThrottle,
        changed_msg: u32,
    }

    impl Settings {
        /// Draws the controls; returns whether any value changed.
        fn controls(&mut self, ui: &mut egui::Ui) -> bool {
            let mut changed = false;
            ui.heading("Mode");
            changed |= ui.radio_value(&mut self.config.mode, OverlayMode::AlertsOnly, "Alerts only").changed();
            changed |= ui.radio_value(&mut self.config.mode, OverlayMode::HudAndAlerts, "HUD + alerts").changed();
            ui.separator();

            let hud = &mut self.config.hud;
            ui.heading("Position");
            egui::Grid::new("corners").show(ui, |ui| {
                for row in [
                    [(Corner::TopLeft, "Top left"), (Corner::TopRight, "Top right")],
                    [(Corner::BottomLeft, "Bottom left"), (Corner::BottomRight, "Bottom right")],
                ] {
                    for (corner, label) in row {
                        changed |= ui.selectable_value(&mut hud.corner, corner, label).changed();
                    }
                    ui.end_row();
                }
            });
            ui.separator();

            ui.heading("Opacity");
            changed |= ui.add(percent_slider(&mut hud.opacity, 0.2..=1.0)).changed();
            ui.heading("Size");
            changed |= ui.add(percent_slider(&mut hud.scale, 0.5..=3.0)).changed();
            changed
        }

        /// Merges the edits onto the file's current contents (the tray may have
        /// switched the mode meanwhile), saves, and tells the running overlay
        /// to reload. A failed save stays pending and is retried on a later frame.
        fn save(&mut self, now: Instant) {
            let disk = config::try_load(&self.path).unwrap_or_else(|_| self.on_disk.clone());
            let merged = apply_edits(disk, &self.on_disk, &self.config);
            match config::save(&self.path, &merged) {
                Ok(()) => {
                    self.throttle.saved(now);
                    self.config = merged.clone();
                    self.on_disk = merged;
                    notify_main_window(self.changed_msg);
                }
                Err(e) => eprintln!("could not save config: {e}"),
            }
        }
    }

    impl eframe::App for Settings {
        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            // Coming back to the window: show changes made from the tray meanwhile.
            let focused = ui.ctx().input(|i| i.focused);
            if focused && !self.focused && self.throttle.wait(Instant::now()).is_none() {
                if let Ok(cfg) = config::try_load(&self.path) {
                    self.config = cfg.clone();
                    self.on_disk = cfg;
                }
            }
            self.focused = focused;
            egui::CentralPanel::default().show(ui, |ui| {
                if self.controls(ui) {
                    self.throttle.mark_dirty();
                }
            });
            let now = Instant::now();
            if self.throttle.due(now) {
                self.save(now);
            }
            if let Some(wait) = self.throttle.wait(now) {
                // Zero only after a failed save: retry later rather than every frame.
                ui.ctx().request_repaint_after(if wait.is_zero() { SAVE_INTERVAL } else { wait });
            }
        }

        fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
            if self.throttle.wait(Instant::now()).is_some() {
                self.save(Instant::now());
            }
        }
    }

    fn percent_slider(value: &mut f32, range: RangeInclusive<f32>) -> egui::Slider<'_> {
        egui::Slider::new(value, range).custom_formatter(|v, _| percent_text(v)).custom_parser(parse_percent)
    }

    /// Does nothing if the overlay isn't running; it reads the file at startup.
    fn notify_main_window(msg: u32) {
        unsafe {
            if let Ok(hwnd) = FindWindowW(&HSTRING::from(MAIN_WINDOW_CLASS), PCWSTR::null()) {
                let _ = PostMessageW(hwnd, msg, WPARAM(0), LPARAM(0));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Corner, OverlayMode};

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn first_change_saves_immediately() {
        let t0 = Instant::now();
        let mut t = SaveThrottle::default();
        t.mark_dirty();
        assert!(t.due(t0));
        t.saved(t0);
        assert!(!t.due(t0));
    }

    #[test]
    fn clean_throttle_is_never_due() {
        let t0 = Instant::now();
        let t = SaveThrottle::default();
        assert!(!t.due(t0));
        assert_eq!(t.wait(t0), None);
    }

    #[test]
    fn changes_within_interval_wait_then_save() {
        let t0 = Instant::now();
        let mut t = SaveThrottle::default();
        t.saved(t0);
        t.mark_dirty();
        assert!(!t.due(t0 + MS(40)));
        assert_eq!(t.wait(t0 + MS(40)), Some(MS(60)));
        assert!(t.due(t0 + MS(100)));
    }

    #[test]
    fn percent_formats_and_parses_fractions() {
        assert_eq!(percent_text(0.9), "90%");
        assert_eq!(percent_text(2.5), "250%");
        assert_eq!(parse_percent("45%"), Some(0.45));
        assert_eq!(parse_percent(" 150 "), Some(1.5));
        assert_eq!(parse_percent("abc"), None);
    }

    #[test]
    fn apply_edits_keeps_changes_made_elsewhere() {
        let before = Config::default();
        let mut edited = before.clone();
        edited.hud.opacity = 0.5;
        // The tray switched the mode while the settings window was open.
        let mut disk = before.clone();
        disk.mode = OverlayMode::AlertsOnly;
        disk.hotkey = "Alt+F9".into();
        let merged = apply_edits(disk, &before, &edited);
        assert_eq!(merged.mode, OverlayMode::AlertsOnly);
        assert_eq!(merged.hotkey, "Alt+F9");
        assert_eq!(merged.hud.opacity, 0.5);
    }

    #[test]
    fn apply_edits_writes_every_edited_field() {
        let before = Config::default();
        let mut edited = before.clone();
        edited.mode = OverlayMode::AlertsOnly;
        edited.hud.corner = Corner::BottomLeft;
        edited.hud.scale = 2.0;
        let merged = apply_edits(before.clone(), &before, &edited);
        assert_eq!(merged.mode, OverlayMode::AlertsOnly);
        assert_eq!(merged.hud.corner, Corner::BottomLeft);
        assert_eq!(merged.hud.scale, 2.0);
        assert_eq!(merged.hud.opacity, before.hud.opacity);
    }

    #[test]
    fn failed_save_stays_due() {
        let t0 = Instant::now();
        let mut t = SaveThrottle::default();
        t.mark_dirty();
        assert!(t.due(t0));
        assert!(t.due(t0 + MS(1)));
    }
}
