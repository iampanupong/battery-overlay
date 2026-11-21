//! Settings window (`--settings`), a separate process so the overlay never loads egui.

use std::time::{Duration, Instant};

use crate::config::{Config, Corner, OverlayMode};

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

/// Registered window message carrying live values while a slider is dragged;
/// `wparam` is `encode_preview(..)`. Nothing is written to disk for it.
pub const PREVIEW_MSG: &str = "BatteryOverlay.Preview";

const CORNERS: [Corner; 4] = [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight];

/// Packs the display settings into one message parameter: bit 0 mode,
/// bits 1–2 corner, bits 3–9 opacity %, bits 10–18 size %.
pub fn encode_preview(cfg: &Config) -> usize {
    let mode = (cfg.mode == OverlayMode::HudAndAlerts) as usize;
    let corner = CORNERS.iter().position(|&c| c == cfg.hud.corner).unwrap_or(0);
    let percent = |v: f32| (v * 100.0).round() as usize;
    mode | corner << 1 | (percent(cfg.hud.opacity) & 0x7f) << 3 | (percent(cfg.hud.scale) & 0x1ff) << 10
}

/// Applies an `encode_preview` value to the mode and HUD settings, clamped
/// to the ranges `Config::sanitized` allows.
pub fn apply_preview(cfg: &mut Config, packed: usize) {
    cfg.mode = if packed & 1 == 1 { OverlayMode::HudAndAlerts } else { OverlayMode::AlertsOnly };
    cfg.hud.corner = CORNERS[packed >> 1 & 3];
    cfg.hud.opacity = ((packed >> 3 & 0x7f) as f32 / 100.0).clamp(0.2, 1.0);
    cfg.hud.scale = ((packed >> 10 & 0x1ff) as f32 / 100.0).clamp(0.5, 3.0);
}

/// "Reset to defaults" in the settings window: only what the window shows.
pub fn reset_display(cfg: &mut Config) {
    let defaults = Config::default();
    cfg.mode = defaults.mode;
    cfg.hud = defaults.hud;
}

#[cfg(windows)]
pub use win::run;

#[cfg(windows)]
mod win {
    use std::ops::RangeInclusive;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Instant;

    use eframe::egui::{
        self, Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Layout, RichText, Sense,
        Stroke, StrokeKind, Vec2,
    };
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, LPARAM, WPARAM};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsIconic, PostMessageW, RegisterWindowMessageW, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    use super::{
        apply_edits, encode_preview, percent_text, reset_display, SaveThrottle, CONFIG_CHANGED_MSG, CORNERS,
        MAIN_WINDOW_CLASS, PREVIEW_MSG, SAVE_INTERVAL, SETTINGS_MUTEX,
    };
    use crate::config::{self, Config, Corner, OverlayMode};

    const TITLE: &str = "Battery Overlay Settings";

    // Dark palette taken from the HUD (overlay/hud.rs) so both read as one app.
    const BG: Color32 = Color32::from_rgb(0x12, 0x14, 0x18);
    const CARD: Color32 = Color32::from_rgb(0x1b, 0x1e, 0x24);
    const CONTROL: Color32 = Color32::from_rgb(0x26, 0x2a, 0x32);
    const CONTROL_HOVER: Color32 = Color32::from_rgb(0x31, 0x36, 0x3f);
    const BORDER: Color32 = Color32::from_rgb(0x2e, 0x33, 0x3b);
    const ACCENT: Color32 = Color32::from_rgb(0x4d, 0xd9, 0x73);
    const ON_ACCENT: Color32 = Color32::from_rgb(0x0b, 0x1a, 0x10);
    const TEXT: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf5);
    const TEXT_DIM: Color32 = Color32::from_rgb(0x9e, 0xa3, 0xad);
    const ERROR: Color32 = Color32::from_rgb(0xff, 0x6b, 0x6b);
    const SEMIBOLD: &str = "semibold";

    /// Shows the settings window until it is closed. If one is already open,
    /// brings it to the front instead.
    pub fn run() {
        // Held until the process exits; the OS releases it.
        let _instance_lock = unsafe { CreateMutexW(None, true, &HSTRING::from(SETTINGS_MUTEX)) };
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            if let Ok(hwnd) = unsafe { FindWindowW(PCWSTR::null(), &HSTRING::from(TITLE)) } {
                unsafe {
                    if IsIconic(hwnd).as_bool() {
                        let _ = ShowWindow(hwnd, SW_RESTORE);
                    }
                    let _ = SetForegroundWindow(hwnd);
                }
            }
            return;
        }

        let path = config::config_path();
        let loaded = config::load(&path);
        let message = |name: &str| unsafe { RegisterWindowMessageW(&HSTRING::from(name)) };
        let app = Settings {
            config: loaded.clone(),
            on_disk: loaded,
            focused: true,
            saved_once: false,
            save_failed: false,
            path,
            throttle: SaveThrottle::default(),
            changed_msg: message(CONFIG_CHANGED_MSG),
            preview_msg: message(PREVIEW_MSG),
        };
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title(TITLE)
                .with_inner_size([360.0, 470.0])
                .with_resizable(false)
                .with_maximize_button(false),
            ..Default::default()
        };
        let created = eframe::run_native(
            TITLE,
            options,
            Box::new(|cc| {
                apply_theme(&cc.egui_ctx);
                Ok(Box::new(app))
            }),
        );
        if let Err(e) = created {
            eprintln!("settings window failed: {e}");
        }
    }

    struct Settings {
        /// What the controls show and edit.
        config: Config,
        /// The config as last loaded or saved; `config` minus this = the user's edits.
        on_disk: Config,
        focused: bool,
        saved_once: bool,
        save_failed: bool,
        path: PathBuf,
        throttle: SaveThrottle,
        changed_msg: u32,
        preview_msg: u32,
    }

    /// What the controls did this frame: `preview` sends the values to the
    /// overlay right away; `commit` also saves them (on release, not mid-drag).
    #[derive(Default)]
    struct Edit {
        preview: bool,
        commit: bool,
    }

    impl Edit {
        fn changed(&mut self, changed: bool) {
            self.preview |= changed;
            self.commit |= changed;
        }

        /// A slider previews every move and commits when let go (or on a
        /// keyboard step, which has no drag).
        fn slider(&mut self, r: &egui::Response) {
            self.preview |= r.changed();
            self.commit |= r.drag_stopped() || (r.changed() && !r.dragged());
        }
    }

    impl Settings {
        fn controls(&mut self, ui: &mut egui::Ui) -> Edit {
            let mut edit = Edit::default();
            card(ui, "Display mode", |ui| {
                edit.changed(segmented(ui, &mut self.config.mode));
                let hint = match self.config.mode {
                    OverlayMode::AlertsOnly => "The overlay appears only when a battery runs low.".to_owned(),
                    OverlayMode::HudAndAlerts => {
                        format!("Always shows the battery HUD; {} hides it.", self.config.hotkey)
                    }
                };
                ui.label(RichText::new(hint).color(TEXT_DIM).size(12.5));
            });
            let hud = &mut self.config.hud;
            card(ui, "Position", |ui| edit.changed(corner_picker(ui, &mut hud.corner)));
            card(ui, "Appearance", |ui| {
                edit.slider(&percent_slider(ui, "Opacity", &mut hud.opacity, 0.2..=1.0));
                ui.add_space(6.0);
                edit.slider(&percent_slider(ui, "Size", &mut hud.scale, 0.5..=3.0));
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new("Reset to defaults").min_size(Vec2::new(0.0, 32.0))).clicked() {
                    reset_display(&mut self.config);
                    edit.changed(true);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let status = if self.save_failed {
                        Some(RichText::new("Couldn't save, retrying").color(ERROR))
                    } else if self.throttle.wait(Instant::now()).is_some() {
                        Some(RichText::new("Saving…").color(TEXT_DIM))
                    } else if self.saved_once {
                        Some(RichText::new("✔ Saved").color(ACCENT))
                    } else {
                        None
                    };
                    if let Some(status) = status {
                        ui.label(status.size(12.5));
                    }
                });
            });
            edit
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
                    self.saved_once = true;
                    self.save_failed = false;
                    self.config = merged.clone();
                    self.on_disk = merged;
                    notify_main_window(self.changed_msg, 0);
                }
                Err(e) => {
                    self.save_failed = true;
                    eprintln!("could not save config: {e}");
                }
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

            let panel = egui::Frame::new().fill(BG).inner_margin(16);
            let edit = egui::CentralPanel::default().frame(panel).show(ui, |ui| self.controls(ui)).inner;
            if edit.preview {
                notify_main_window(self.preview_msg, encode_preview(&self.config));
            }
            if edit.commit {
                self.throttle.mark_dirty();
            }
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

    /// A rounded section with a small caps label, like a Windows settings card.
    fn card<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
        let frame = egui::Frame::new().fill(CARD).stroke(Stroke::new(1.0, BORDER)).corner_radius(10).inner_margin(14);
        let inner = frame
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(title.to_uppercase()).family(semibold()).size(11.5).color(TEXT_DIM));
                ui.add_space(2.0);
                add(ui)
            })
            .inner;
        ui.add_space(10.0);
        inner
    }

    /// Two-option segmented control; returns whether the value changed.
    fn segmented(ui: &mut egui::Ui, mode: &mut OverlayMode) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = (ui.available_width() - 4.0) / 2.0;
            for (option, label) in [(OverlayMode::AlertsOnly, "Alerts only"), (OverlayMode::HudAndAlerts, "HUD + alerts")] {
                let selected = *mode == option;
                let text = RichText::new(label).color(if selected { ON_ACCENT } else { TEXT });
                let button = egui::Button::new(if selected { text.family(semibold()) } else { text })
                    .fill(if selected { ACCENT } else { CONTROL })
                    .min_size(Vec2::new(width, 34.0));
                if ui.add(button).clicked() && !selected {
                    *mode = option;
                    changed = true;
                }
            }
        });
        changed
    }

    /// A miniature screen; clicking a corner moves the HUD there, drawn as a
    /// small green bar in that corner. Returns whether the value changed.
    fn corner_picker(ui: &mut egui::Ui, value: &mut Corner) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            let (screen, _) = ui.allocate_exact_size(Vec2::new(176.0, 99.0), Sense::hover());
            let painter = ui.painter().clone();
            painter.rect_filled(screen, 8, BG);
            painter.rect_stroke(screen, 8, Stroke::new(1.0, BORDER), StrokeKind::Inside);
            for (i, &corner) in CORNERS.iter().enumerate() {
                let inset = screen.shrink(8.0);
                let (left, top) = matches_corner(corner);
                let anchor = Align2([if left { Align::Min } else { Align::Max }, if top { Align::Min } else { Align::Max }]);
                let hit = anchor.align_size_within_rect(Vec2::new(52.0, 32.0), screen);
                let response = ui
                    .interact(hit, ui.scope_id().with(("corner", i)), Sense::click())
                    .on_hover_text(corner_name(corner));
                let selected = *value == corner;
                let hud = anchor.align_size_within_rect(Vec2::new(36.0, 14.0), inset);
                if selected {
                    painter.rect_filled(hud, 3, ACCENT);
                } else {
                    let fill = if response.hovered() { CONTROL_HOVER } else { CONTROL };
                    painter.rect_filled(hud, 3, fill);
                }
                if response.has_focus() {
                    painter.rect_stroke(hud.expand(3.0), 5, Stroke::new(1.5, TEXT), StrokeKind::Outside);
                }
                if response.clicked() && !selected {
                    *value = corner;
                    changed = true;
                }
            }
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.add_space(30.0);
                ui.label(RichText::new(corner_name(*value)).family(semibold()).color(TEXT));
                ui.label(RichText::new("Click a corner").color(TEXT_DIM).size(12.5));
            });
        });
        changed
    }

    /// (is left, is top)
    fn matches_corner(corner: Corner) -> (bool, bool) {
        match corner {
            Corner::TopLeft => (true, true),
            Corner::TopRight => (false, true),
            Corner::BottomLeft => (true, false),
            Corner::BottomRight => (false, false),
        }
    }

    fn corner_name(corner: Corner) -> &'static str {
        match corner {
            Corner::TopLeft => "Top left",
            Corner::TopRight => "Top right",
            Corner::BottomLeft => "Bottom left",
            Corner::BottomRight => "Bottom right",
        }
    }

    /// Label and value on one line, full-width slider below.
    fn percent_slider(ui: &mut egui::Ui, label: &str, value: &mut f32, range: RangeInclusive<f32>) -> egui::Response {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(percent_text(*value as f64)).family(semibold()).color(TEXT));
            });
        });
        ui.spacing_mut().slider_width = ui.available_width();
        ui.add(egui::Slider::new(value, range).show_value(false).trailing_fill(true))
    }

    fn semibold() -> FontFamily {
        FontFamily::Name(SEMIBOLD.into())
    }

    /// Segoe UI (the Windows UI font) when available, the HUD palette, and
    /// larger targets than egui's defaults.
    fn apply_theme(ctx: &egui::Context) {
        let fonts_dir = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
        let mut fonts = FontDefinitions::default();
        let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
        let mut load = |name: &str, file: &str| -> bool {
            match std::fs::read(fonts_dir.join(file)) {
                Ok(bytes) => {
                    fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes)));
                    true
                }
                Err(_) => false,
            }
        };
        let regular = load("segoe", "segoeui.ttf");
        let has_semibold = load("segoe-semibold", "seguisb.ttf");
        let family = |first: Option<&str>| first.map(String::from).into_iter().chain(fallbacks.iter().cloned()).collect();
        fonts.families.insert(FontFamily::Proportional, family(regular.then_some("segoe")));
        fonts.families.insert(
            semibold(),
            family(if has_semibold { Some("segoe-semibold") } else { regular.then_some("segoe") }),
        );
        ctx.set_fonts(fonts);

        ctx.set_theme(egui::Theme::Dark);
        ctx.all_styles_mut(|style| {
            for (text_style, size) in [
                (egui::TextStyle::Body, 14.0),
                (egui::TextStyle::Button, 14.0),
                (egui::TextStyle::Small, 11.5),
                (egui::TextStyle::Heading, 18.0),
            ] {
                style.text_styles.insert(text_style, FontId::proportional(size));
            }
            let spacing = &mut style.spacing;
            spacing.item_spacing = Vec2::new(8.0, 8.0);
            spacing.button_padding = Vec2::new(12.0, 6.0);
            spacing.interact_size.y = 32.0;
            spacing.slider_rail_height = 6.0;

            let v = &mut style.visuals;
            v.panel_fill = BG;
            v.window_fill = BG;
            v.extreme_bg_color = CONTROL;
            v.override_text_color = None;
            v.selection.bg_fill = ACCENT;
            v.selection.stroke = Stroke::new(1.0, ON_ACCENT);
            v.slider_trailing_fill = true;
            let w = &mut v.widgets;
            w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
            for (state, fill) in [(&mut w.inactive, CONTROL), (&mut w.hovered, CONTROL_HOVER), (&mut w.active, CONTROL_HOVER)] {
                state.bg_fill = fill;
                state.weak_bg_fill = fill;
                state.corner_radius = 6.into();
                state.fg_stroke = Stroke::new(1.5, TEXT);
                state.expansion = 0.0;
            }
            w.inactive.bg_stroke = Stroke::NONE;
            w.hovered.bg_stroke = Stroke::new(1.0, BORDER);
            // Keyboard focus and press get a visible accent ring.
            w.active.bg_stroke = Stroke::new(1.5, ACCENT);
        });
    }

    /// Does nothing if the overlay isn't running; it reads the file at startup.
    fn notify_main_window(msg: u32, wparam: usize) {
        unsafe {
            if let Ok(hwnd) = FindWindowW(&HSTRING::from(MAIN_WINDOW_CLASS), PCWSTR::null()) {
                let _ = PostMessageW(hwnd, msg, WPARAM(wparam), LPARAM(0));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn percent_formats_fractions() {
        assert_eq!(percent_text(0.9), "90%");
        assert_eq!(percent_text(2.5), "250%");
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
    fn preview_round_trips_display_settings() {
        let mut cfg = Config::default();
        cfg.mode = OverlayMode::AlertsOnly;
        cfg.hud.corner = Corner::BottomLeft;
        cfg.hud.opacity = 0.2;
        cfg.hud.scale = 3.0;
        let mut out = Config::default();
        out.hotkey = "Alt+F9".into();
        apply_preview(&mut out, encode_preview(&cfg));
        assert_eq!(out.mode, OverlayMode::AlertsOnly);
        assert_eq!(out.hud.corner, Corner::BottomLeft);
        assert_eq!(out.hud.opacity, 0.2);
        assert_eq!(out.hud.scale, 3.0);
        assert_eq!(out.hotkey, "Alt+F9");
    }

    #[test]
    fn preview_rounds_to_whole_percents_and_clamps() {
        let mut cfg = Config::default();
        cfg.hud.opacity = 0.876;
        cfg.hud.scale = 1.234;
        let mut out = Config::default();
        apply_preview(&mut out, encode_preview(&cfg));
        assert_eq!(out.hud.opacity, 0.88);
        assert_eq!(out.hud.scale, 1.23);
        // A garbage message must still land inside the sanitized ranges.
        apply_preview(&mut out, usize::MAX);
        assert!((0.2..=1.0).contains(&out.hud.opacity));
        assert!((0.5..=3.0).contains(&out.hud.scale));
    }

    #[test]
    fn reset_restores_display_defaults_only() {
        let mut cfg = Config::default();
        cfg.mode = OverlayMode::AlertsOnly;
        cfg.hud.opacity = 0.3;
        cfg.hotkey = "Alt+F9".into();
        reset_display(&mut cfg);
        assert_eq!(cfg.mode, Config::default().mode);
        assert_eq!(cfg.hud, Config::default().hud);
        assert_eq!(cfg.hotkey, "Alt+F9");
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
