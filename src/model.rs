use crate::config::Thresholds;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Mouse,
    Keyboard,
    Headset,
    Controller,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    /// Battery percentage, or `None` when the device is connected but doesn't report a level.
    pub level: Option<u8>,
    pub charging: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Unknown,
    Ok,
    Warning,
    Critical,
}

pub fn severity(level: Option<u8>, t: &Thresholds) -> Severity {
    match level {
        None => Severity::Unknown,
        Some(l) if l <= t.critical => Severity::Critical,
        Some(l) if l <= t.warning => Severity::Warning,
        Some(_) => Severity::Ok,
    }
}

/// Placeholder devices used until the real providers land (phase 2).
pub fn fake_devices() -> Vec<Device> {
    let dev = |id: &str, name: &str, kind, level, charging| Device {
        id: id.into(),
        name: name.into(),
        kind,
        level,
        charging,
    };
    vec![
        dev("fake-mouse", "Logitech G Pro X Superlight", DeviceKind::Mouse, Some(76), false),
        dev("fake-pad", "Xbox Wireless Controller", DeviceKind::Controller, Some(18), false),
        dev("fake-headset", "Arctis Nova 7", DeviceKind::Headset, Some(9), false),
        dev("fake-kb", "Keychron K2", DeviceKind::Keyboard, Some(54), true),
        dev("fake-other", "Lamzu Atlantis", DeviceKind::Other, None, false),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_uses_inclusive_thresholds() {
        let t = Thresholds::default(); // warning 20, critical 10
        assert_eq!(severity(None, &t), Severity::Unknown);
        assert_eq!(severity(Some(100), &t), Severity::Ok);
        assert_eq!(severity(Some(21), &t), Severity::Ok);
        assert_eq!(severity(Some(20), &t), Severity::Warning);
        assert_eq!(severity(Some(11), &t), Severity::Warning);
        assert_eq!(severity(Some(10), &t), Severity::Critical);
        assert_eq!(severity(Some(0), &t), Severity::Critical);
    }
}
