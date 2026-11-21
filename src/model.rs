use crate::config::Thresholds;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Mouse,
    Keyboard,
    Headset,
    Controller,
    Other,
}

impl DeviceKind {
    /// Guess from a product name, for sources that don't report a type.
    pub fn from_name(name: &str) -> Self {
        let n = name.to_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
        // Checked first: "PRO X Wireless Gaming Headset" must not match "g pro".
        if has(&["headset", "headphone"]) {
            DeviceKind::Headset
        } else if has(&["mouse", "mx master", "mx anywhere", "trackball", "trackpad"])
            || has(&["viper", "deathadder", "basilisk", "naga", "orochi", "mamba", "lancehead", "atheris", "cobra"])
            || has(&["superlight", "g pro", "g502", "g903", "g305", "harpoon", "dark core", "katar", "m65", "sabre", "ironclaw"])
        {
            DeviceKind::Mouse
        } else if has(&["keyboard", "keys", "keychron", "mx mechanical", "blackwidow", "huntsman", "k57", "k63", "k100"]) {
            DeviceKind::Keyboard
        } else if has(&["controller", "gamepad", "dualsense", "dualshock", "joy-con", "8bitdo"]) {
            DeviceKind::Controller
        } else if has(&[
            "buds", "airpods", "earbuds", "wh-", "wf-", "arctis", "nova", "jabra", "kraken",
            "barracuda", "blackshark", "virtuoso", "void", "hs80", "hs70",
        ]) {
            DeviceKind::Headset
        } else {
            DeviceKind::Other
        }
    }
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

/// Placeholder devices for `--demo`, to check the overlay without real hardware.
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

    #[test]
    fn kind_from_name() {
        assert_eq!(DeviceKind::from_name("MX Master 3"), DeviceKind::Mouse);
        assert_eq!(DeviceKind::from_name("Razer Viper Ultimate"), DeviceKind::Mouse);
        assert_eq!(DeviceKind::from_name("Keychron K2"), DeviceKind::Keyboard);
        assert_eq!(DeviceKind::from_name("Arctis Nova 7"), DeviceKind::Headset);
        assert_eq!(DeviceKind::from_name("CORSAIR VIRTUOSO RGB WIRELESS XT"), DeviceKind::Headset);
        assert_eq!(DeviceKind::from_name("G PRO X Wireless Gaming Headset"), DeviceKind::Headset);
        assert_eq!(DeviceKind::from_name("Speaker"), DeviceKind::Other);
    }
}
