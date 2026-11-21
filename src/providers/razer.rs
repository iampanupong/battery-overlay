//! Razer wireless mice and keyboards (and their dongles and docks) over
//! Razer's 90-byte feature-report protocol.
//!
//! Report layout after the report ID (0): status, transaction ID, two bytes
//! of remaining-packet count, protocol type, argument size, command class,
//! command ID, 80 argument bytes, XOR checksum of bytes 2..88, reserved.
//! Devices only answer requests carrying their transaction ID, which differs
//! by model, so the known ones are tried in turn. Razer headsets use other
//! protocols and aren't covered.

use super::hid::FeatureChannel;

/// Report ID byte plus the 90-byte report.
pub const REPORT_LEN: usize = 91;
const TRANSACTION_IDS: [u8; 3] = [0x1f, 0x3f, 0xff];
const CLASS_POWER: u8 = 0x07;
const CMD_BATTERY_LEVEL: u8 = 0x80;
const CMD_CHARGING: u8 = 0x84;

const STATUS_BUSY: u8 = 0x01;
const STATUS_OK: u8 = 0x02;
const BUSY_RETRIES: usize = 5;

pub fn request(transaction_id: u8, class: u8, command: u8, arg_size: u8) -> [u8; REPORT_LEN] {
    let mut r = [0u8; REPORT_LEN];
    r[2] = transaction_id;
    r[6] = arg_size;
    r[7] = class;
    r[8] = command;
    r[89] = checksum(&r);
    r
}

fn checksum(r: &[u8; REPORT_LEN]) -> u8 {
    r[3..89].iter().fold(0, |acc, b| acc ^ b)
}

/// The argument bytes of a successful reply to `req`, `Err(true)` while the
/// device is still busy, `Err(false)` for any other failure.
pub fn parse_reply<'a>(req: &[u8; REPORT_LEN], r: &'a [u8]) -> Result<&'a [u8], bool> {
    if r.len() < REPORT_LEN || r[7] != req[7] || r[8] != req[8] {
        return Err(false);
    }
    match r[1] {
        STATUS_OK => Ok(&r[9..89]),
        STATUS_BUSY => Err(true),
        _ => Err(false),
    }
}

fn call(dev: &mut dyn FeatureChannel, req: &[u8; REPORT_LEN]) -> Option<Vec<u8>> {
    if !dev.set_feature(req) {
        return None;
    }
    for _ in 0..BUSY_RETRIES {
        dev.pause();
        let r = dev.get_feature(0, REPORT_LEN)?;
        match parse_reply(req, &r) {
            Ok(args) => return Some(args.to_vec()),
            Err(true) => continue,
            Err(false) => return None,
        }
    }
    None
}

/// Battery level (0–255 on the wire) scaled to a percentage.
pub fn percent(raw: u8) -> u8 {
    ((raw as u16 * 100 + 127) / 255) as u8
}

/// Battery percentage and charging state, or `None` if the device doesn't
/// answer (e.g. a dongle whose mouse is asleep, or a wired-only device).
pub fn query(dev: &mut dyn FeatureChannel) -> Option<(u8, bool)> {
    let (tid, args) = TRANSACTION_IDS
        .iter()
        .find_map(|&tid| Some((tid, call(dev, &request(tid, CLASS_POWER, CMD_BATTERY_LEVEL, 2))?)))?;
    // A sleeping device behind a dongle reads as 0; don't raise a false alert.
    let level = percent(*args.get(1)?);
    if level == 0 {
        return None;
    }
    let charging = call(dev, &request(tid, CLASS_POWER, CMD_CHARGING, 2)).is_some_and(|a| a.get(1) == Some(&1));
    Some((level, charging))
}

#[cfg(windows)]
pub use win::poll;

#[cfg(windows)]
mod win {
    use std::collections::BTreeSet;

    use super::{query, REPORT_LEN};
    use crate::model::{Device, DeviceKind};
    use crate::providers::hid::{self, clean_product_name, HidDevice, HidInfo};

    /// The first collection of each product that answers wins, so a device
    /// exposing the protocol on several interfaces appears once.
    pub fn poll(collections: &[HidInfo]) -> Vec<Device> {
        let mut done = BTreeSet::new();
        let mut devices = Vec::new();
        for c in collections.iter().filter(|c| c.vid == hid::RAZER && c.feature_len as usize == REPORT_LEN) {
            if done.contains(&c.pid) {
                continue;
            }
            let Some(mut dev) = HidDevice::open_for_features(c) else { continue };
            if let Some((level, charging)) = query(&mut dev) {
                done.insert(c.pid);
                let name = c.product.as_deref().map(clean_product_name).unwrap_or_else(|| "Razer device".into());
                devices.push(Device {
                    id: format!("razer:{:04x}", c.pid),
                    kind: DeviceKind::from_name(&name),
                    name,
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

    /// A fake device that answers only `tid`, is busy once per request, and
    /// reports `raw` battery and `charging`.
    struct Fake {
        tid: u8,
        raw: u8,
        charging: u8,
        last: Option<[u8; REPORT_LEN]>,
        busy: bool,
    }

    impl FeatureChannel for Fake {
        fn set_feature(&mut self, report: &[u8]) -> bool {
            self.last = Some(report.try_into().unwrap());
            self.busy = true;
            true
        }

        fn get_feature(&mut self, _id: u8, len: usize) -> Option<Vec<u8>> {
            let req = self.last?;
            let mut r = req.to_vec();
            r.resize(len, 0);
            r[1] = if req[2] != self.tid {
                0x04 // timeout: wrong transaction ID
            } else if std::mem::take(&mut self.busy) {
                STATUS_BUSY
            } else {
                STATUS_OK
            };
            r[10] = if req[8] == CMD_BATTERY_LEVEL { self.raw } else { self.charging };
            Some(r)
        }
    }

    #[test]
    fn request_layout_and_checksum() {
        let r = request(0x1f, 0x07, 0x80, 0x02);
        assert_eq!(&r[..9], &[0, 0, 0x1f, 0, 0, 0, 0x02, 0x07, 0x80]);
        assert_eq!(r[89], 0x02 ^ 0x07 ^ 0x80);
    }

    #[test]
    fn finds_transaction_id_and_retries_while_busy() {
        let mut dev = Fake { tid: 0x3f, raw: 128, charging: 1, last: None, busy: false };
        assert_eq!(query(&mut dev), Some((50, true)));
    }

    #[test]
    fn sleeping_device_is_skipped() {
        let mut dev = Fake { tid: 0xff, raw: 0, charging: 0, last: None, busy: false };
        assert_eq!(query(&mut dev), None);
    }

    #[test]
    fn rejects_replies_to_other_commands() {
        let req = request(0x1f, 0x07, 0x80, 2);
        let mut r = req;
        r[1] = STATUS_OK;
        r[8] = 0x84;
        assert_eq!(parse_reply(&req, &r), Err(false));
    }

    #[test]
    fn percent_scaling() {
        assert_eq!(percent(255), 100);
        assert_eq!(percent(0), 0);
        assert_eq!(percent(13), 5);
    }
}
