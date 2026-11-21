//! Xbox controllers through XInput: the Xbox Wireless Adapter, USB, and
//! Bluetooth.
//!
//! XInput only reports four coarse battery levels, so they are mapped to
//! representative percentages chosen to line up with the default alert
//! thresholds: "low" warns and "empty" is critical.

use crate::model::{Device, DeviceKind};

pub const TYPE_DISCONNECTED: u8 = 0;
pub const TYPE_WIRED: u8 = 1;

/// Percentage for an XInput battery type and level; `None` for empty slots and
/// wired controllers, which have no battery to show.
pub fn level(battery_type: u8, battery_level: u8) -> Option<u8> {
    match battery_type {
        TYPE_DISCONNECTED | TYPE_WIRED => None,
        _ => Some(match battery_level {
            0 => 5,
            1 => 20,
            2 => 60,
            _ => 100,
        }),
    }
}

pub fn device(slot: u32, level: u8) -> Device {
    Device {
        id: format!("xinput:{slot}"),
        name: "Xbox Wireless Controller".into(),
        kind: DeviceKind::Controller,
        level: Some(level),
        charging: false,
    }
}

/// A controller paired over Bluetooth is also seen by XInput. The Bluetooth
/// provider's reading is a real percentage, so one XInput controller is
/// dropped for each Xbox controller Bluetooth already reports.
pub fn without_bluetooth_duplicates(bluetooth: &[Device], mut xinput: Vec<Device>) -> Vec<Device> {
    let on_bluetooth = bluetooth
        .iter()
        .filter(|d| d.kind == DeviceKind::Controller && d.name.to_lowercase().contains("xbox"))
        .count();
    xinput.truncate(xinput.len().saturating_sub(on_bluetooth));
    xinput
}

#[cfg(windows)]
pub use win::poll;

#[cfg(windows)]
mod win {
    use windows::Win32::UI::Input::XboxController::{
        XInputGetBatteryInformation, BATTERY_DEVTYPE_GAMEPAD, XINPUT_BATTERY_INFORMATION, XUSER_MAX_COUNT,
    };

    use crate::model::Device;

    pub fn poll() -> Vec<Device> {
        (0..XUSER_MAX_COUNT)
            .filter_map(|slot| {
                let mut info = XINPUT_BATTERY_INFORMATION::default();
                if unsafe { XInputGetBatteryInformation(slot, BATTERY_DEVTYPE_GAMEPAD, &mut info) } != 0 {
                    return None;
                }
                Some(super::device(slot, super::level(info.BatteryType.0, info.BatteryLevel.0)?))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Thresholds;
    use crate::model::{severity, Severity};

    #[test]
    fn maps_coarse_levels_onto_alert_thresholds() {
        let t = Thresholds::default();
        assert_eq!(level(TYPE_DISCONNECTED, 3), None);
        assert_eq!(level(TYPE_WIRED, 3), None);
        assert_eq!(severity(level(2, 0), &t), Severity::Critical);
        assert_eq!(severity(level(3, 1), &t), Severity::Warning);
        assert_eq!(severity(level(2, 2), &t), Severity::Ok);
        assert_eq!(level(255, 3), Some(100));
    }

    #[test]
    fn drops_controllers_bluetooth_already_reports() {
        let bt = |name: &str, kind| Device { id: "bt:x".into(), name: name.into(), kind, level: Some(80), charging: false };
        let bluetooth = [bt("Xbox Wireless Controller", DeviceKind::Controller), bt("Xbox speaker", DeviceKind::Other)];
        let xinput = vec![device(0, 60), device(1, 20)];
        let kept = without_bluetooth_duplicates(&bluetooth, xinput);
        assert_eq!(kept.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["xinput:0"]);
        assert_eq!(without_bluetooth_duplicates(&bluetooth, vec![]), vec![]);
    }
}
