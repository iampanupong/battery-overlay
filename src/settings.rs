//! Settings window (`--settings`), a separate process so the overlay never loads egui.

use std::ops::RangeInclusive;
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

/// Value at fraction `t` (0–1) along a slider, snapped to a whole percent
/// so the label, the preview and the saved value all agree.
pub fn slider_value(t: f32, range: RangeInclusive<f32>) -> f32 {
    let (min, max) = (*range.start(), *range.end());
    snap_percent(min + t.clamp(0.0, 1.0) * (max - min)).clamp(min, max)
}

/// One keyboard step: `delta` whole percents, stopping at the ends.
pub fn slider_step(value: f32, delta: i32, range: RangeInclusive<f32>) -> f32 {
    ((((value * 100.0).round() as i32 + delta) as f32) / 100.0).clamp(*range.start(), *range.end())
}

/// Where `value` sits along the track, 0–1.
pub fn slider_fraction(value: f32, range: RangeInclusive<f32>) -> f32 {
    let (min, max) = (*range.start(), *range.end());
    ((value - min) / (max - min)).clamp(0.0, 1.0)
}

fn snap_percent(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
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
        self, Align, Align2, Color32, EventFilter, FontData, FontDefinitions, FontFamily, FontId, Key, Layout,
        Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2,
    };
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, LPARAM, WPARAM};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsIconic, PostMessageW, RegisterWindowMessageW, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    use super::{
        apply_edits, encode_preview, percent_text, reset_display, slider_fraction, slider_step, slider_value,
        SaveThrottle, CONFIG_CHANGED_MSG, CORNERS, MAIN_WINDOW_CLASS, PREVIEW_MSG, SAVE_INTERVAL, SETTINGS_MUTEX,
    };
    use crate::config::{self, Config, Corner, HudConfig, OverlayMode};

    const TITLE: &str = "Battery Overlay Settings";

    // Monochrome graphite: white marks the selection, so the HUD's own
    // green/amber/red are the only colors on screen.
    const BG: Color32 = Color32::from_rgb(0x10, 0x10, 0x10);
    const SURFACE: Color32 = Color32::from_rgb(0x18, 0x18, 0x18);
    const LINE: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
    const TRACK: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);
    const INK: Color32 = Color32::from_rgb(0xfa, 0xfa, 0xfa);
    const DIM: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x8a);
    const OK: Color32 = Color32::from_rgb(0x6e, 0xe7, 0xa0);
    const ERROR: Color32 = Color32::from_rgb(0xff, 0x6b, 0x6b);
    const HOVER_WASH: Color32 = Color32::from_rgba_premultiplied(6, 6, 6, 6);

    // Outfit (SIL Open Font License, see assets/fonts/OFL.txt).
    const OUTFIT_REGULAR: &[u8] = include_bytes!("../assets/fonts/Outfit-Regular.ttf");
    const OUTFIT_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Outfit-Medium.ttf");
    const MEDIUM: &str = "medium";

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
                .with_inner_size([360.0, 500.0])
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
        /// click or keyboard step, which have no drag).
        fn slider(&mut self, r: &egui::Response) {
            self.preview |= r.changed();
            self.commit |= r.drag_stopped() || (r.changed() && !r.dragged());
        }
    }

    impl Settings {
        fn controls(&mut self, ui: &mut egui::Ui) -> Edit {
            let mut edit = Edit::default();
            label_row(ui, "Display mode", None);
            edit.changed(segmented(ui, &mut self.config.mode));
            divider(ui);

            label_row(ui, "Position", Some(corner_name(self.config.hud.corner)));
            let hud_shown = self.config.mode == OverlayMode::HudAndAlerts;
            edit.changed(corner_picker(ui, &mut self.config.hud, hud_shown));
            divider(ui);

            let hud = &mut self.config.hud;
            label_row(ui, "Opacity", Some(&percent_text(hud.opacity as f64)));
            edit.slider(&slim_slider(ui, &mut hud.opacity, 0.2..=1.0));
            ui.add_space(6.0);
            label_row(ui, "Size", Some(&percent_text(hud.scale as f64)));
            edit.slider(&slim_slider(ui, &mut hud.scale, 0.5..=3.0));
            ui.add_space(14.0);

            ui.horizontal(|ui| {
                let reset = egui::Button::new(RichText::new("Reset to defaults").color(DIM).size(13.0).underline())
                    .frame(false);
                if ui.add(reset).clicked() {
                    reset_display(&mut self.config);
                    edit.changed(true);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let status = if self.save_failed {
                        Some(RichText::new("couldn't save, retrying").color(ERROR))
                    } else if self.throttle.wait(Instant::now()).is_some() {
                        Some(RichText::new("saving…").color(DIM))
                    } else if self.saved_once {
                        Some(RichText::new("● saved").color(OK))
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

            let panel = egui::Frame::new().fill(BG).inner_margin(egui::Margin { left: 20, right: 20, top: 18, bottom: 18 });
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

    /// Dim label on the left, current value on the right.
    fn label_row(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(DIM).size(13.5));
            if let Some(value) = value {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(value).family(medium()).color(INK).size(13.5));
                });
            }
        });
    }

    /// Hairline between sections, with equal space above and below.
    fn divider(ui: &mut egui::Ui) {
        ui.add_space(10.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
        ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, LINE));
        ui.add_space(10.0);
    }

    /// Outlined two-option switch with a white thumb that slides to the
    /// selected side. Returns whether the value changed.
    fn segmented(ui: &mut egui::Ui, mode: &mut OverlayMode) -> bool {
        let options = [(OverlayMode::AlertsOnly, "Alerts only"), (OverlayMode::HudAndAlerts, "HUD + alerts")];
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::hover());
        let painter = ui.painter().clone();
        painter.rect_stroke(rect, 7, Stroke::new(1.0, LINE), StrokeKind::Inside);

        let inner = rect.shrink(2.0);
        let half = Vec2::new(inner.width() / 2.0, inner.height());
        let selected = options.iter().position(|(m, _)| m == mode).unwrap_or(0) as f32;
        let thumb_x = ui.ctx().animate_value_with_time(ui.scope_id().with("segmented"), selected, 0.15);
        painter.rect_filled(Rect::from_min_size(inner.min + Vec2::new(thumb_x * half.x, 0.0), half), 5, INK);

        let mut changed = false;
        for (i, (option, label)) in options.into_iter().enumerate() {
            let cell = Rect::from_min_size(inner.min + Vec2::new(i as f32 * half.x, 0.0), half);
            let response = ui.interact(cell, ui.scope_id().with(("segment", i)), Sense::click());
            let is_selected = *mode == option;
            let (family, color) = if is_selected {
                (medium(), BG)
            } else {
                (FontFamily::Proportional, if response.hovered() { INK } else { DIM })
            };
            painter.text(cell.center(), Align2::CENTER_CENTER, label, FontId::new(14.0, family), color);
            if response.has_focus() {
                painter.rect_stroke(cell.expand(1.0), 6, Stroke::new(1.5, DIM), StrokeKind::Outside);
            }
            if response.clicked() && !is_selected {
                *mode = option;
                changed = true;
            }
        }
        changed
    }

    /// A miniature screen: click a quarter to move the HUD to that corner. The
    /// small HUD inside follows the current opacity and size. Returns whether
    /// the corner changed.
    fn corner_picker(ui: &mut egui::Ui, hud: &mut HudConfig, hud_shown: bool) -> bool {
        let width = ui.available_width();
        let (screen, _) = ui.allocate_exact_size(Vec2::new(width, (width * 9.0 / 16.0).round()), Sense::hover());
        let painter = ui.painter().clone();
        painter.rect_filled(screen, 6, SURFACE);

        let mut changed = false;
        let half = screen.size() / 2.0;
        for (i, &corner) in CORNERS.iter().enumerate() {
            let (left, top) = corner_side(corner);
            let offset = Vec2::new(if left { 0.0 } else { half.x }, if top { 0.0 } else { half.y });
            let quarter = Rect::from_min_size(screen.min + offset, half);
            let response = ui
                .interact(quarter, ui.scope_id().with(("corner", i)), Sense::click())
                .on_hover_text(corner_name(corner));
            if response.hovered() && hud.corner != corner {
                painter.rect_filled(quarter, 6, HOVER_WASH);
            }
            if response.has_focus() {
                painter.rect_stroke(quarter.shrink(2.0), 5, Stroke::new(1.5, DIM), StrokeKind::Inside);
            }
            if response.clicked() && hud.corner != corner {
                hud.corner = corner;
                changed = true;
            }
        }
        paint_mini_hud(&painter, screen, hud, hud_shown);
        changed
    }

    /// A small stand-in for the real HUD, three sample devices.
    fn paint_mini_hud(painter: &egui::Painter, screen: Rect, hud: &HudConfig, shown: bool) {
        const ROWS: [(&str, u8, Color32); 3] = [
            ("Headset", 82, Color32::from_rgb(0x4d, 0xd9, 0x73)),
            ("Mouse", 46, Color32::from_rgb(0x4d, 0xd9, 0x73)),
            ("Controller", 14, Color32::from_rgb(0xff, 0x52, 0x52)),
        ];
        let s = 0.55 * hud.scale;
        let size = Vec2::new(78.0 * s, (8.0 + 12.0 * ROWS.len() as f32) * s);
        let (left, top) = corner_side(hud.corner);
        let inset = 8.0;
        let x = if left { screen.left() + inset } else { screen.right() - inset - size.x };
        let y = if top { screen.top() + inset } else { screen.bottom() - inset - size.y };
        let panel = Rect::from_min_size(Pos2::new(x, y), size);
        let alpha = if shown { hud.opacity } else { 0.12 };
        let fade = |c: Color32| c.gamma_multiply(alpha);

        painter.rect_filled(panel, 3.0 * s, fade(Color32::from_rgb(0x12, 0x14, 0x1a)));
        for (i, (name, level, color)) in ROWS.iter().enumerate() {
            let cy = panel.top() + (4.0 + 6.0 + 12.0 * i as f32) * s;
            let icon = Rect::from_min_size(Pos2::new(panel.left() + 5.0 * s, cy - 2.5 * s), Vec2::new(11.0 * s, 5.0 * s));
            painter.rect_stroke(icon, 1.0, Stroke::new(1.0, fade(DIM)), StrokeKind::Inside);
            let fill = Rect::from_min_size(icon.min, Vec2::new(icon.width() * *level as f32 / 100.0, icon.height()));
            painter.rect_filled(fill.shrink(1.0), 0.0, fade(*color));
            let font = FontId::new(7.0 * s, FontFamily::Proportional);
            painter.text(Pos2::new(icon.right() + 4.0 * s, cy), Align2::LEFT_CENTER, name, font.clone(), fade(INK));
            painter.text(Pos2::new(panel.right() - 5.0 * s, cy), Align2::RIGHT_CENTER, format!("{level}%"), font, fade(INK));
        }
    }

    /// Thin rounded track with a white fill and a slim handle; snaps to whole
    /// percents. Arrow keys step 1 % when focused.
    fn slim_slider(ui: &mut egui::Ui, value: &mut f32, range: RangeInclusive<f32>) -> egui::Response {
        let (rect, mut response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::click_and_drag());
        let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width() - 8.0, 6.0));

        if response.is_pointer_button_down_on() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let new = slider_value((pointer.x - track.left()) / track.width(), range.clone());
                if new != *value {
                    *value = new;
                    response.mark_changed();
                }
            }
        }
        if response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(response.id, EventFilter { horizontal_arrows: true, ..Default::default() })
            });
            let delta = ui.input(|i| {
                i.key_pressed(Key::ArrowRight) as i32 + i.key_pressed(Key::ArrowUp) as i32
                    - i.key_pressed(Key::ArrowLeft) as i32
                    - i.key_pressed(Key::ArrowDown) as i32
            });
            if delta != 0 {
                let new = slider_step(*value, delta, range.clone());
                if new != *value {
                    *value = new;
                    response.mark_changed();
                }
            }
        }

        let painter = ui.painter();
        let x = track.left() + track.width() * slider_fraction(*value, range);
        painter.rect_filled(track, 3, TRACK);
        painter.rect_filled(Rect::from_min_max(track.min, Pos2::new(x, track.bottom())), 3, INK);
        if response.hovered() || response.dragged() || response.has_focus() {
            let handle = Rect::from_center_size(Pos2::new(x, track.center().y), Vec2::new(4.0, 16.0));
            painter.rect_filled(handle, 2, INK);
        }
        if response.has_focus() {
            painter.rect_stroke(track.expand(4.0), 6, Stroke::new(1.5, DIM), StrokeKind::Outside);
        }
        response
    }

    /// (is left, is top)
    fn corner_side(corner: Corner) -> (bool, bool) {
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

    fn medium() -> FontFamily {
        FontFamily::Name(MEDIUM.into())
    }

    /// Bundled Outfit for all text (egui's own fonts stay as fallbacks for
    /// symbols), the graphite palette, and larger targets than egui's defaults.
    fn apply_theme(ctx: &egui::Context) {
        let mut fonts = FontDefinitions::default();
        let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
        fonts.font_data.insert("outfit".into(), Arc::new(FontData::from_static(OUTFIT_REGULAR)));
        fonts.font_data.insert("outfit-medium".into(), Arc::new(FontData::from_static(OUTFIT_MEDIUM)));
        let family = |first: &str| std::iter::once(first.to_owned()).chain(fallbacks.iter().cloned()).collect();
        fonts.families.insert(FontFamily::Proportional, family("outfit"));
        fonts.families.insert(medium(), family("outfit-medium"));
        ctx.set_fonts(fonts);

        ctx.set_theme(egui::Theme::Dark);
        ctx.all_styles_mut(|style| {
            for (text_style, size) in [
                (egui::TextStyle::Body, 14.0),
                (egui::TextStyle::Button, 14.0),
                (egui::TextStyle::Small, 12.0),
                (egui::TextStyle::Heading, 18.0),
            ] {
                style.text_styles.insert(text_style, FontId::proportional(size));
            }
            style.spacing.item_spacing = Vec2::new(8.0, 10.0);
            style.spacing.button_padding = Vec2::new(0.0, 4.0);

            let v = &mut style.visuals;
            v.panel_fill = BG;
            v.window_fill = BG;
            v.selection.bg_fill = INK;
            v.selection.stroke = Stroke::new(1.0, BG);
            let w = &mut v.widgets;
            w.noninteractive.fg_stroke = Stroke::new(1.0, INK);
            for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                state.bg_fill = SURFACE;
                state.weak_bg_fill = SURFACE;
                state.bg_stroke = Stroke::NONE;
                state.fg_stroke = Stroke::new(1.0, INK);
                state.corner_radius = 6.into();
                state.expansion = 0.0;
            }
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
    fn slider_value_snaps_to_whole_percents_inside_range() {
        assert_eq!(slider_value(0.0, 0.2..=1.0), 0.2);
        assert_eq!(slider_value(1.0, 0.5..=3.0), 3.0);
        assert_eq!(slider_value(0.5, 0.2..=1.0), 0.6);
        assert_eq!(slider_value(0.333, 0.5..=3.0), 1.33);
        assert_eq!(slider_value(-2.0, 0.2..=1.0), 0.2);
        assert_eq!(slider_value(7.0, 0.2..=1.0), 1.0);
    }

    #[test]
    fn slider_step_moves_one_percent_and_stops_at_ends() {
        assert_eq!(slider_step(0.9, 1, 0.2..=1.0), 0.91);
        assert_eq!(slider_step(0.9, -1, 0.2..=1.0), 0.89);
        assert_eq!(slider_step(1.0, 1, 0.2..=1.0), 1.0);
        assert_eq!(slider_step(0.2, -1, 0.2..=1.0), 0.2);
    }

    #[test]
    fn slider_fraction_is_position_along_track() {
        assert_eq!(slider_fraction(0.2, 0.2..=1.0), 0.0);
        assert!((slider_fraction(0.6, 0.2..=1.0) - 0.5).abs() < 1e-6);
        assert_eq!(slider_fraction(3.0, 0.5..=3.0), 1.0);
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
