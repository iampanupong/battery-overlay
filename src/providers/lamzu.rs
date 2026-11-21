//! Lamzu wireless mice through their 2.4 GHz dongle (Maya X verified).
//!
//! Lamzu publishes no protocol; this layout was read from a USB capture of
//! the Maya X web driver. The dongle's settings collection takes 64-byte
//! feature reports (report ID 0). Requests are `00 00 target len class cmd
//! params…`; replies are `A1 00 target len class cmd` followed by `len` data
//! bytes. Target 0x02 is the mouse behind the dongle. Only the battery
//! command (class 0x00, cmd 0x83) is ever sent, so the mouse's settings are
//! never touched. Its data is a charging flag, then the level in percent;
//! the flag has only been seen as 0 (not charging) and is unverified.

use super::hid::FeatureChannel;

/// Report ID byte plus the 64-byte report.
pub const REPORT_LEN: usize = 65;
const TARGET_MOUSE: u8 = 0x02;
const CLASS_STATUS: u8 = 0x00;
const CMD_BATTERY: u8 = 0x83;
const STATUS_OK: u8 = 0xa1;
/// The feature buffer can still hold the reply to another program's request
/// (e.g. the web driver), so re-read until ours shows up.
const RETRIES: usize = 5;

pub fn request(target: u8, class: u8, cmd: u8, len: u8) -> [u8; REPORT_LEN] {
    let mut r = [0u8; REPORT_LEN];
    r[3] = target;
    r[4] = len;
    r[5] = class;
    r[6] = cmd;
    r
}

/// The data bytes of a successful reply to `req`, or `None` if `r` isn't one.
pub fn parse_reply<'a>(req: &[u8; REPORT_LEN], r: &'a [u8]) -> Option<&'a [u8]> {
    let len = req[4] as usize;
    if r.len() < 7 + len || r[1] != STATUS_OK || r[3..7] != req[3..7] {
        return None;
    }
    Some(&r[7..7 + len])
}

/// Battery percentage and charging state, or `None` if the mouse doesn't
/// answer (asleep, off or out of range).
pub fn query(dev: &mut dyn FeatureChannel) -> Option<(u8, bool)> {
    let req = request(TARGET_MOUSE, CLASS_STATUS, CMD_BATTERY, 2);
    if !dev.set_feature(&req) {
        return None;
    }
    for _ in 0..RETRIES {
        dev.pause();
        let r = dev.get_feature(0, REPORT_LEN)?;
        if let Some(data) = parse_reply(&req, &r) {
            // A sleeping mouse reads as 0; don't raise a false alert.
            let level = data[1].min(100);
            return (level > 0).then_some((level, data[0] == 1));
        }
    }
    None
}

#[cfg(windows)]
pub use win::poll;

#[cfg(windows)]
mod win {
    use std::collections::BTreeSet;

    use super::{query, REPORT_LEN};
    use crate::model::{Device, DeviceKind};
    use crate::providers::hid::{self, clean_product_name, HidDevice, HidInfo};

    pub fn poll(collections: &[HidInfo]) -> Vec<Device> {
        let mut done = BTreeSet::new();
        let mut devices = Vec::new();
        // PIDs differ between dongles (0x001c and 0x001e seen), so match the
        // protocol collection instead.
        for c in collections.iter().filter(|c| c.vid == hid::LAMZU && c.feature_len as usize == REPORT_LEN) {
            if done.contains(&c.pid) {
                continue;
            }
            let Some(mut dev) = HidDevice::open_for_features(c) else { continue };
            if let Some((level, charging)) = query(&mut dev) {
                done.insert(c.pid);
                devices.push(Device {
                    id: format!("lamzu:{:04x}", c.pid),
                    name: c.product.as_deref().map(clean_product_name).unwrap_or_else(|| "Lamzu mouse".into()),
                    kind: DeviceKind::Mouse,
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

    /// A fake dongle that answers each get with the next of `replies` (report
    /// bytes after the ID; the last one repeats).
    struct Fake {
        replies: Vec<Vec<u8>>,
        sent: Vec<Vec<u8>>,
        gets: usize,
    }

    impl Fake {
        fn answering(replies: &[&[u8]]) -> Self {
            Fake { replies: replies.iter().map(|r| r.to_vec()).collect(), sent: Vec::new(), gets: 0 }
        }
    }

    impl FeatureChannel for Fake {
        fn set_feature(&mut self, report: &[u8]) -> bool {
            self.sent.push(report.to_vec());
            true
        }

        fn get_feature(&mut self, report_id: u8, len: usize) -> Option<Vec<u8>> {
            let reply = &self.replies[self.gets.min(self.replies.len() - 1)];
            self.gets += 1;
            let mut r = vec![report_id];
            r.extend_from_slice(reply);
            r.resize(len, 0);
            Some(r)
        }
    }

    const BATTERY_100: &[u8] = &[0xa1, 0x00, 0x02, 0x02, 0x00, 0x83, 0x00, 0x64];
    const CONNECTION: &[u8] = &[0xa1, 0x00, 0x02, 0x01, 0x00, 0x82, 0x02];

    #[test]
    fn battery_request_matches_capture() {
        let r = request(TARGET_MOUSE, CLASS_STATUS, CMD_BATTERY, 2);
        assert_eq!(&r[..7], &[0x00, 0x00, 0x00, 0x02, 0x02, 0x00, 0x83]);
        assert!(r[7..].iter().all(|&b| b == 0));
    }

    #[test]
    fn reads_captured_reply() {
        // The web driver showed 100 %, not charging.
        let mut dev = Fake::answering(&[BATTERY_100]);
        assert_eq!(query(&mut dev), Some((100, false)));
        assert_eq!(dev.sent, [request(TARGET_MOUSE, CLASS_STATUS, CMD_BATTERY, 2).to_vec()]);
    }

    #[test]
    fn charging_flag() {
        let mut dev = Fake::answering(&[&[0xa1, 0x00, 0x02, 0x02, 0x00, 0x83, 0x01, 0x37]]);
        assert_eq!(query(&mut dev), Some((55, true)));
    }

    #[test]
    fn level_zero_or_error_status_hides_device() {
        let mut dev = Fake::answering(&[&[0xa1, 0x00, 0x02, 0x02, 0x00, 0x83, 0x00, 0x00]]);
        assert_eq!(query(&mut dev), None);
        let mut dev = Fake::answering(&[&[0xa2, 0x00, 0x02, 0x02, 0x00, 0x83, 0x00, 0x64]]);
        assert_eq!(query(&mut dev), None);
    }

    #[test]
    fn skips_stale_reply_then_reads_battery() {
        let mut dev = Fake::answering(&[CONNECTION, BATTERY_100]);
        assert_eq!(query(&mut dev), Some((100, false)));
        assert_eq!(dev.gets, 2);
    }

    #[test]
    fn gives_up_after_retries() {
        let mut dev = Fake::answering(&[CONNECTION]);
        assert_eq!(query(&mut dev), None);
        assert_eq!(dev.gets, RETRIES);
    }

    #[test]
    fn short_reply_is_rejected() {
        let req = request(TARGET_MOUSE, CLASS_STATUS, CMD_BATTERY, 2);
        assert_eq!(parse_reply(&req, &[0x00, 0xa1, 0x00, 0x02, 0x02]), None);
        assert_eq!(parse_reply(&req, &[0x00, 0xa1, 0x00, 0x02, 0x02, 0x00, 0x83, 0x00]), None);
    }
}
