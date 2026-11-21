//! HUD layout and styling, kept free of Win32 so it can be unit-tested anywhere.
//! All metrics are in DIPs; multiply by the scale factor to get pixels.

use crate::config::Corner;
use crate::model::Severity;

pub const PADDING: f32 = 10.0;
pub const ROW_HEIGHT: f32 = 26.0;
pub const WIDTH: f32 = 260.0;
pub const ICON_W: f32 = 20.0;
pub const GAP: f32 = 6.0;
pub const PCT_W: f32 = 40.0;
pub const BAR_W: f32 = 30.0;
pub const BAR_H: f32 = 14.0;
pub const NUB_W: f32 = 2.0;
pub const CORNER_RADIUS: f32 = 8.0;
pub const FONT_SIZE: f32 = 13.0;
pub const ICON_FONT_SIZE: f32 = 14.0;
pub const BOLT_FONT_SIZE: f32 = 10.0;

pub type Rgb = (f32, f32, f32);
pub const BACKGROUND: Rgb = (0.07, 0.08, 0.10);
pub const BACKGROUND_ALPHA: f32 = 0.82;
pub const TEXT: Rgb = (0.95, 0.95, 0.97);
pub const TEXT_DIM: Rgb = (0.62, 0.64, 0.68);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Pixel size of the HUD for `rows` devices; an empty list still gets one
/// row for the "No devices found" message.
pub fn hud_size(rows: usize, scale: f32) -> (u32, u32) {
    let rows = rows.max(1) as f32;
    let w = (WIDTH * scale).ceil() as u32;
    let h = ((PADDING * 2.0 + ROW_HEIGHT * rows) * scale).ceil() as u32;
    (w.max(1), h.max(1))
}

/// Top-left pixel position of the HUD inside `monitor`.
pub fn position(corner: Corner, monitor: Rect, size: (u32, u32), margin: i32) -> (i32, i32) {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let left = monitor.left + margin;
    let right = monitor.right - margin - w;
    let top = monitor.top + margin;
    let bottom = monitor.bottom - margin - h;
    match corner {
        Corner::TopLeft => (left, top),
        Corner::TopRight => (right, top),
        Corner::BottomLeft => (left, bottom),
        Corner::BottomRight => (right, bottom),
    }
}

pub fn severity_color(s: Severity) -> Rgb {
    match s {
        Severity::Ok => (0.30, 0.85, 0.45),
        Severity::Warning => (1.00, 0.74, 0.20),
        Severity::Critical => (1.00, 0.32, 0.32),
        Severity::Unknown => TEXT_DIM,
    }
}

pub fn level_text(level: Option<u8>) -> String {
    match level {
        Some(l) => format!("{l}%"),
        None => "—".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: Rect = Rect { left: 0, top: 0, right: 1920, bottom: 1080 };

    #[test]
    fn size_grows_with_rows_and_scale() {
        assert_eq!(hud_size(0, 1.0), hud_size(1, 1.0));
        assert_eq!(hud_size(1, 1.0), (260, 46));
        assert_eq!(hud_size(3, 1.0), (260, 98));
        assert_eq!(hud_size(3, 1.5), (390, 147));
    }

    #[test]
    fn positions_in_each_corner() {
        let size = (200, 100);
        assert_eq!(position(Corner::TopLeft, MONITOR, size, 16), (16, 16));
        assert_eq!(position(Corner::TopRight, MONITOR, size, 16), (1704, 16));
        assert_eq!(position(Corner::BottomLeft, MONITOR, size, 16), (16, 964));
        assert_eq!(position(Corner::BottomRight, MONITOR, size, 16), (1704, 964));
    }

    #[test]
    fn positions_relative_to_secondary_monitor_origin() {
        let monitor = Rect { left: -1280, top: -200, right: 0, bottom: 824 };
        assert_eq!(position(Corner::TopLeft, monitor, (100, 50), 0), (-1280, -200));
        assert_eq!(position(Corner::BottomRight, monitor, (100, 50), 0), (-100, 774));
    }

    #[test]
    fn level_text_formats_unknown() {
        assert_eq!(level_text(Some(7)), "7%");
        assert_eq!(level_text(None), "—");
    }
}
