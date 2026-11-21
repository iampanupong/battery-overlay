//! Settings window (`--settings`), a separate process so the overlay never loads egui.

use std::time::{Duration, Instant};

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
    fn failed_save_stays_due() {
        let t0 = Instant::now();
        let mut t = SaveThrottle::default();
        t.mark_dirty();
        assert!(t.due(t0));
        assert!(t.due(t0 + MS(1)));
    }
}
