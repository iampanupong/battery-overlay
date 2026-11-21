use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::{fs, io};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayMode {
    /// Tray icon only; the overlay appears just for low-battery alerts.
    AlertsOnly,
    /// Always-on HUD in a screen corner, plus alerts.
    HudAndAlerts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudConfig {
    pub corner: Corner,
    /// 0.2 – 1.0
    pub opacity: f32,
    /// Extra scale on top of the monitor DPI, 0.5 – 3.0
    pub scale: f32,
    /// Distance from the screen edges in DIPs.
    pub margin: i32,
}

impl Default for HudConfig {
    fn default() -> Self {
        Self { corner: Corner::TopRight, opacity: 0.9, scale: 1.0, margin: 16 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    pub warning: u8,
    pub critical: u8,
    /// Repeat the critical alert every N minutes; 0 disables repeats.
    pub critical_repeat_minutes: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self { warning: 20, critical: 10, critical_repeat_minutes: 10 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub mode: OverlayMode,
    /// Toggles the HUD, e.g. "Ctrl+Shift+B".
    pub hotkey: String,
    pub poll_interval_secs: u64,
    pub thresholds: Thresholds,
    pub hud: HudConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: OverlayMode::HudAndAlerts,
            hotkey: "Ctrl+Shift+B".into(),
            poll_interval_secs: 60,
            thresholds: Thresholds::default(),
            hud: HudConfig::default(),
        }
    }
}

impl Config {
    /// Clamps hand-edited values into ranges the app can work with.
    pub fn sanitized(mut self) -> Self {
        let finite_or = |v: f32, d: f32| if v.is_finite() { v } else { d };
        self.hud.opacity = finite_or(self.hud.opacity, 0.9).clamp(0.2, 1.0);
        self.hud.scale = finite_or(self.hud.scale, 1.0).clamp(0.5, 3.0);
        self.hud.margin = self.hud.margin.clamp(0, 500);
        self.poll_interval_secs = self.poll_interval_secs.clamp(5, 3600);
        self.thresholds.warning = self.thresholds.warning.min(100);
        self.thresholds.critical = self.thresholds.critical.min(self.thresholds.warning);
        self
    }
}

/// HUD visibility after the config is reloaded: a hotkey-hidden HUD stays
/// hidden unless the mode itself changed, which resets it to match the mode.
pub fn hud_visible_after_reload(old: OverlayMode, new: OverlayMode, visible: bool) -> bool {
    if old == new {
        visible
    } else {
        new == OverlayMode::HudAndAlerts
    }
}

pub fn config_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("BatteryOverlay")
        .join("config.toml")
}

/// Loads the config, writing defaults on first run. A malformed file is left
/// untouched (so the user's edits aren't lost) and defaults are used instead.
pub fn load(path: &Path) -> Config {
    match try_load(path) {
        Ok(cfg) => cfg,
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            eprintln!("config {} is invalid, using defaults: {e}", path.display());
            Config::default()
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let cfg = Config::default();
            if let Err(e) = save(path, &cfg) {
                eprintln!("could not write default config {}: {e}", path.display());
            }
            cfg
        }
        Err(e) => {
            eprintln!("could not read config {}: {e}", path.display());
            Config::default()
        }
    }
}

/// Reads the config without falling back to defaults, for reloads where
/// keeping the current settings beats resetting them. A malformed file is
/// reported as `InvalidData`.
pub fn try_load(path: &Path) -> io::Result<Config> {
    let text = fs::read_to_string(path)?;
    toml::from_str::<Config>(&text)
        .map(Config::sanitized)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn save(path: &Path, cfg: &Config) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = toml::to_string_pretty(cfg).map_err(io::Error::other)?;
    // Write then rename so a crash mid-write never leaves a truncated config.
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("battery_overlay_test_{}_{name}", std::process::id()))
            .join("config.toml")
    }

    #[test]
    fn round_trips_through_toml() {
        let mut cfg = Config::default();
        cfg.mode = OverlayMode::AlertsOnly;
        cfg.hud.corner = Corner::BottomLeft;
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), cfg);
    }

    #[test]
    fn partial_file_fills_in_defaults() {
        let cfg: Config = toml::from_str("mode = \"alerts_only\"\n[hud]\ncorner = \"bottom_right\"").unwrap();
        assert_eq!(cfg.mode, OverlayMode::AlertsOnly);
        assert_eq!(cfg.hud.corner, Corner::BottomRight);
        assert_eq!(cfg.hud.opacity, HudConfig::default().opacity);
        assert_eq!(cfg.thresholds, Thresholds::default());
    }

    #[test]
    fn sanitize_clamps_out_of_range_values() {
        let mut cfg = Config::default();
        cfg.hud.opacity = 5.0;
        cfg.hud.scale = f32::NAN;
        cfg.poll_interval_secs = 0;
        cfg.thresholds.warning = 150;
        cfg.thresholds.critical = 120;
        let cfg = cfg.sanitized();
        assert_eq!(cfg.hud.opacity, 1.0);
        assert_eq!(cfg.hud.scale, 1.0);
        assert_eq!(cfg.poll_interval_secs, 5);
        assert_eq!(cfg.thresholds.warning, 100);
        assert_eq!(cfg.thresholds.critical, 100);
    }

    #[test]
    fn reload_keeps_hotkey_visibility_unless_mode_changes() {
        use OverlayMode::*;
        assert!(!hud_visible_after_reload(HudAndAlerts, HudAndAlerts, false));
        assert!(hud_visible_after_reload(AlertsOnly, HudAndAlerts, false));
        assert!(!hud_visible_after_reload(HudAndAlerts, AlertsOnly, true));
        assert!(hud_visible_after_reload(AlertsOnly, AlertsOnly, true));
    }

    #[test]
    fn load_creates_default_file_when_missing() {
        let path = temp_path("missing");
        let _ = fs::remove_dir_all(path.parent().unwrap());
        assert_eq!(load(&path), Config::default());
        assert!(path.exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn load_keeps_malformed_file_and_uses_defaults() {
        let path = temp_path("malformed");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "mode = 42").unwrap();
        assert_eq!(load(&path), Config::default());
        assert_eq!(fs::read_to_string(&path).unwrap(), "mode = 42");
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn try_load_reports_missing_and_malformed_files() {
        let path = temp_path("try_load");
        let _ = fs::remove_dir_all(path.parent().unwrap());
        assert!(try_load(&path).is_err());
        assert!(!path.exists());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "mode = 42").unwrap();
        assert!(try_load(&path).is_err());
        fs::write(&path, "[hud]\nopacity = 7.0").unwrap();
        assert_eq!(try_load(&path).unwrap().hud.opacity, 1.0);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn save_then_load() {
        let path = temp_path("save");
        let mut cfg = Config::default();
        cfg.hotkey = "Alt+F9".into();
        save(&path, &cfg).unwrap();
        assert_eq!(load(&path), cfg);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
