//! SteelSeries Arctis wireless headsets (best-effort).
//!
//! Each family answers a short output report on one USB interface with an
//! input report carrying the battery. Request bytes and offsets come from
//! public protocol notes and haven't been checked against every model; check
//! a new model with `--probe`.

use std::time::Duration;

use super::hid::{transact, Channel};

const TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reading {
    /// The base station answered but the headset is off or out of range.
    Offline,
    Battery { level: u8, charging: bool },
}

pub struct Family {
    pub pids: &'static [u16],
    pub interface: u8,
    pub request: &'static [u8],
    /// Parses the reply, or `None` if the report isn't the reply.
    pub parse: fn(&[u8]) -> Option<Reading>,
}

pub const FAMILIES: &[Family] = &[
    // Arctis 7 (2017, 2019), Arctis Pro (2019): level 0–100 in byte 2.
    Family { pids: &[0x1260, 0x12ad, 0x1252], interface: 5, request: &[0x06, 0x18], parse: parse_arctis_7 },
    // Arctis Nova 7 / 7X / 7P and variants: level 0–4 in byte 2, status in byte 3.
    Family {
        pids: &[0x2202, 0x2206, 0x220a, 0x223a, 0x227a],
        interface: 3,
        request: &[0x00, 0xb0],
        parse: parse_nova_7,
    },
    // Arctis Nova Pro Wireless: level 0–8 in byte 6, status in byte 15.
    Family { pids: &[0x12e0, 0x12e5], interface: 4, request: &[0x06, 0xb0], parse: parse_nova_pro },
];

pub fn family(pid: u16) -> Option<&'static Family> {
    FAMILIES.iter().find(|f| f.pids.contains(&pid))
}

pub fn parse_arctis_7(r: &[u8]) -> Option<Reading> {
    (r.len() >= 3 && r[0] == 0x06 && r[1] == 0x18).then(|| Reading::Battery { level: r[2].min(100), charging: false })
}

pub fn parse_nova_7(r: &[u8]) -> Option<Reading> {
    if r.len() < 4 || r[1] != 0xb0 {
        return None;
    }
    Some(match r[3] {
        0x00 => Reading::Offline,
        status => Reading::Battery { level: r[2].min(4) * 25, charging: status == 0x01 },
    })
}

pub fn parse_nova_pro(r: &[u8]) -> Option<Reading> {
    if r.len() < 16 || r[0] != 0x06 || r[1] != 0xb0 {
        return None;
    }
    Some(match r[15] {
        0x01 => Reading::Offline,
        status => Reading::Battery { level: (r[6].min(8) as u16 * 100 / 8) as u8, charging: status == 0x02 },
    })
}

/// Battery percentage and charging state; `None` when offline or silent.
pub fn query(ch: &mut dyn Channel, family: &Family) -> Option<(u8, bool)> {
    match transact(ch, family.request, TIMEOUT, family.parse)? {
        Reading::Offline => None,
        Reading::Battery { level, charging } => Some((level, charging)),
    }
}

#[cfg(windows)]
pub use win::poll;

#[cfg(windows)]
mod win {
    use std::collections::BTreeSet;

    use super::{family, query};
    use crate::model::{Device, DeviceKind};
    use crate::providers::hid::{self, clean_product_name, HidDevice, HidInfo};

    pub fn poll(collections: &[HidInfo]) -> Vec<Device> {
        let mut done = BTreeSet::new();
        let mut devices = Vec::new();
        for c in collections.iter().filter(|c| c.vid == hid::STEELSERIES && c.output_len > 0 && c.input_len > 0) {
            let Some(f) = family(c.pid) else { continue };
            if c.interface != Some(f.interface) || done.contains(&c.pid) {
                continue;
            }
            let Some(mut dev) = HidDevice::open(c) else { continue };
            if let Some((level, charging)) = query(&mut dev, f) {
                done.insert(c.pid);
                devices.push(Device {
                    id: format!("steelseries:{:04x}", c.pid),
                    name: c.product.as_deref().map(clean_product_name).unwrap_or_else(|| "SteelSeries headset".into()),
                    kind: DeviceKind::Headset,
                    level: Some(level),
                    charging,
                });
            }
        }
        devices
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::hid::MockChannel;

    #[test]
    fn every_pid_belongs_to_one_family() {
        let mut pids: Vec<u16> = FAMILIES.iter().flat_map(|f| f.pids.iter().copied()).collect();
        let n = pids.len();
        pids.sort();
        pids.dedup();
        assert_eq!(pids.len(), n);
    }

    #[test]
    fn arctis_7() {
        let mut ch = MockChannel::new(|_| vec![vec![0x01, 0x00], vec![0x06, 0x18, 72, 0, 0, 0, 0, 0]]);
        assert_eq!(query(&mut ch, family(0x12ad).unwrap()), Some((72, false)));
        assert_eq!(ch.written[0], [0x06, 0x18]);
    }

    #[test]
    fn nova_7_levels_and_offline() {
        assert_eq!(parse_nova_7(&[0x00, 0xb0, 3, 0x01]), Some(Reading::Battery { level: 75, charging: true }));
        assert_eq!(parse_nova_7(&[0x00, 0xb0, 2, 0x03]), Some(Reading::Battery { level: 50, charging: false }));
        assert_eq!(parse_nova_7(&[0x00, 0xb0, 4, 0x00]), Some(Reading::Offline));
        let mut ch = MockChannel::new(|_| vec![vec![0x00, 0xb0, 4, 0x00]]);
        assert_eq!(query(&mut ch, family(0x2202).unwrap()), None);
    }

    #[test]
    fn nova_pro() {
        let mut r = vec![0u8; 16];
        r[0] = 0x06;
        r[1] = 0xb0;
        r[6] = 6;
        r[15] = 0x02;
        assert_eq!(parse_nova_pro(&r), Some(Reading::Battery { level: 75, charging: true }));
        r[15] = 0x01;
        assert_eq!(parse_nova_pro(&r), Some(Reading::Offline));
    }
}
