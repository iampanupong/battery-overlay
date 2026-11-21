//! Corsair wireless devices that speak Corsair's newer ("Bragi") protocol,
//! directly or through a Slipstream dongle (best-effort).
//!
//! Requests are `[report 0, target, command, property, …]` on the vendor
//! collection (usage page 0xFF42). Target 0x08 is the device on the cable,
//! 0x09 the wireless device behind a dongle. Replies echo the command, then a
//! status byte (0 = OK), then the value little-endian. Only "get property" is
//! used, so iCUE's lighting and modes are never changed. Older Corsair
//! headsets (Void) use another protocol and aren't covered.

use std::time::Duration;

use super::hid::{transact, Channel};

pub const USAGE_PAGE: u16 = 0xff42;
pub const TARGET_WIRED: u8 = 0x08;
pub const TARGET_WIRELESS: u8 = 0x09;
const CMD_GET: u8 = 0x02;
const PROP_BATTERY_LEVEL: u8 = 0x0f;
const PROP_BATTERY_STATUS: u8 = 0x10;
const STATUS_CHARGING: u16 = 0x01;
const TIMEOUT: Duration = Duration::from_millis(500);

pub fn request(target: u8, property: u8) -> [u8; 5] {
    [0x00, target, CMD_GET, property, 0x00]
}

/// The property value from a reply to a get request.
pub fn parse_reply(r: &[u8]) -> Option<Option<u16>> {
    if r.len() < 5 || r[1] != CMD_GET {
        return None;
    }
    Some((r[2] == 0).then(|| u16::from_le_bytes([r[3], r[4]])))
}

fn get(ch: &mut dyn Channel, target: u8, property: u8) -> Option<u16> {
    transact(ch, &request(target, property), TIMEOUT, parse_reply)?
}

/// Battery percentage and charging state of the device at `target`.
pub fn query(ch: &mut dyn Channel, target: u8) -> Option<(u8, bool)> {
    // The level is in tenths of a percent.
    let permille = get(ch, target, PROP_BATTERY_LEVEL)?;
    let level = (permille.min(1000) / 10) as u8;
    if level == 0 {
        return None;
    }
    let charging = get(ch, target, PROP_BATTERY_STATUS) == Some(STATUS_CHARGING);
    Some((level, charging))
}

#[cfg(windows)]
pub use win::poll;

#[cfg(windows)]
mod win {
    use std::collections::BTreeSet;

    use super::{query, TARGET_WIRED, TARGET_WIRELESS, USAGE_PAGE};
    use crate::model::{Device, DeviceKind};
    use crate::providers::hid::{self, clean_product_name, HidDevice, HidInfo};

    pub fn poll(collections: &[HidInfo]) -> Vec<Device> {
        let mut done = BTreeSet::new();
        let mut devices = Vec::new();
        for c in collections.iter().filter(|c| c.vid == hid::CORSAIR && c.usage_page == USAGE_PAGE && c.output_len > 0) {
            if done.contains(&c.pid) {
                continue;
            }
            let Some(mut dev) = HidDevice::open(c) else { continue };
            for target in [TARGET_WIRELESS, TARGET_WIRED] {
                let Some((level, charging)) = query(&mut dev, target) else { continue };
                done.insert(c.pid);
                let name = c.product.as_deref().map(clean_product_name).unwrap_or_else(|| "Corsair device".into());
                devices.push(Device {
                    id: format!("corsair:{:04x}:{target}", c.pid),
                    kind: DeviceKind::from_name(&name),
                    name,
                    level: Some(level),
                    charging,
                });
                break;
            }
        }
        devices
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::hid::MockChannel;

    /// A dongle whose wireless device is at 64.3 % and charging; the wired
    /// target answers with an error status.
    fn dongle(req: &[u8]) -> Vec<Vec<u8>> {
        let value: u16 = match req[3] {
            PROP_BATTERY_LEVEL => 643,
            _ => 1,
        };
        let status = if req[1] == TARGET_WIRELESS { 0 } else { 0x05 };
        let [lo, hi] = value.to_le_bytes();
        vec![vec![0x00, 0x01, 0x00, 0, 0], vec![0x00, CMD_GET, status, lo, hi, 0, 0]]
    }

    #[test]
    fn reads_wireless_device_behind_dongle() {
        let mut ch = MockChannel::new(dongle);
        assert_eq!(query(&mut ch, TARGET_WIRELESS), Some((64, true)));
        assert_eq!(ch.written[0], [0x00, 0x09, 0x02, 0x0f, 0x00]);
        assert_eq!(query(&mut ch, TARGET_WIRED), None);
    }

    #[test]
    fn zero_level_is_treated_as_asleep() {
        let mut ch = MockChannel::new(|_| vec![vec![0x00, CMD_GET, 0, 0, 0]]);
        assert_eq!(query(&mut ch, TARGET_WIRELESS), None);
    }
}
