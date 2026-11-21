//! Logitech devices over HID++ 2.0: Lightspeed/Unifying/Bolt receivers (up
//! to six paired devices each) and devices plugged in by cable.
//!
//! HID++ lives on vendor collections (usage page 0xFF00): a short report
//! (ID 0x10, 7 bytes) and a long one (ID 0x11, 20 bytes). Requests are sent
//! short; replies come back long, while receiver errors (e.g. "no device in
//! that slot") come back short, so both collections are read. Our software ID
//! keeps our replies apart from G Hub's.

use std::time::Duration;

use super::hid::{transact, Channel};
use crate::model::DeviceKind;

pub const SHORT: u8 = 0x10;
pub const LONG: u8 = 0x11;
/// Device index of a device plugged in directly (or of the receiver itself).
pub const DIRECT: u8 = 0xff;
pub const RECEIVER_SLOTS: std::ops::RangeInclusive<u8> = 1..=6;
/// Low nibble of byte 3; any non-zero value that other software isn't using.
const SW_ID: u8 = 0x0b;
const TIMEOUT: Duration = Duration::from_millis(500);

const ROOT: u8 = 0x00;
const FEATURE_NAME: u16 = 0x0005;
const FEATURE_BATTERY_STATUS: u16 = 0x1000;
const FEATURE_BATTERY_VOLTAGE: u16 = 0x1001;
const FEATURE_UNIFIED_BATTERY: u16 = 0x1004;

#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// Response parameters (bytes after the function byte).
    Ok(Vec<u8>),
    /// HID++ error code.
    Err(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub name: Option<String>,
    pub kind: Option<DeviceKind>,
    pub level: u8,
    pub charging: bool,
}

pub fn request_report(device: u8, feature: u8, function: u8, params: &[u8]) -> [u8; 7] {
    let mut r = [SHORT, device, feature, function << 4 | SW_ID, 0, 0, 0];
    let n = params.len().min(3);
    r[4..4 + n].copy_from_slice(&params[..n]);
    r
}

/// Matches an input report to our request. HID++ 2.0 errors use feature
/// index 0xFF and HID++ 1.0 (receiver) errors use 0x8F; both then echo the
/// request's feature and function bytes and carry the error code.
pub fn parse_reply(r: &[u8], device: u8, feature: u8, function: u8) -> Option<Reply> {
    if r.len() < 6 || !matches!(r[0], SHORT | LONG) || r[1] != device {
        return None;
    }
    let fsw = function << 4 | SW_ID;
    if r[2] == feature && r[3] == fsw {
        Some(Reply::Ok(r[4..].to_vec()))
    } else if matches!(r[2], 0xff | 0x8f) && r[3] == feature && r[4] == fsw {
        Some(Reply::Err(r[5]))
    } else {
        None
    }
}

fn call(ch: &mut dyn Channel, device: u8, feature: u8, function: u8, params: &[u8]) -> Option<Vec<u8>> {
    let req = request_report(device, feature, function, params);
    match transact(ch, &req, TIMEOUT, |r| parse_reply(r, device, feature, function))? {
        Reply::Ok(p) => Some(p),
        Reply::Err(_) => None,
    }
}

/// HID++ 2.0 major version. `None` for HID++ 1.0 (receivers), empty slots
/// and devices that are switched off.
pub fn ping(ch: &mut dyn Channel, device: u8) -> Option<u8> {
    call(ch, device, ROOT, 1, &[0, 0, 0x5a])?.first().copied().filter(|&v| v >= 2)
}

fn feature_index(ch: &mut dyn Channel, device: u8, feature: u16) -> Option<u8> {
    call(ch, device, ROOT, 0, &feature.to_be_bytes())?.first().copied().filter(|&i| i != 0)
}

/// Reads one device's battery, name and type. `None` if it's offline or has
/// no battery feature we understand.
pub fn query(ch: &mut dyn Channel, device: u8) -> Option<Info> {
    ping(ch, device)?;
    let (level, charging) = battery(ch, device)?;
    let (name, kind) = match feature_index(ch, device, FEATURE_NAME) {
        Some(i) => (name(ch, device, i), call(ch, device, i, 2, &[]).and_then(|p| kind_from_type(*p.first()?))),
        None => (None, None),
    };
    Some(Info { name, kind, level, charging })
}

fn battery(ch: &mut dyn Channel, device: u8) -> Option<(u8, bool)> {
    if let Some(i) = feature_index(ch, device, FEATURE_UNIFIED_BATTERY) {
        return parse_unified_battery(&call(ch, device, i, 1, &[])?);
    }
    if let Some(i) = feature_index(ch, device, FEATURE_BATTERY_STATUS) {
        return parse_battery_status(&call(ch, device, i, 0, &[])?);
    }
    let i = feature_index(ch, device, FEATURE_BATTERY_VOLTAGE)?;
    parse_battery_voltage(&call(ch, device, i, 0, &[])?)
}

fn name(ch: &mut dyn Channel, device: u8, index: u8) -> Option<String> {
    let len = (*call(ch, device, index, 0, &[])?.first()? as usize).min(64);
    let mut bytes = Vec::with_capacity(len);
    while bytes.len() < len {
        let chunk = call(ch, device, index, 1, &[bytes.len() as u8])?;
        if chunk.is_empty() {
            break;
        }
        let take = chunk.len().min(len - bytes.len());
        bytes.extend_from_slice(&chunk[..take]);
    }
    let name = String::from_utf8_lossy(&bytes).trim_end_matches('\0').trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Feature 0x1004 GetStatus: state of charge %, level flags, charging status.
/// Devices that only report coarse levels send 0% and set a level flag.
pub fn parse_unified_battery(p: &[u8]) -> Option<(u8, bool)> {
    let &[soc, flags, status, ..] = p else { return None };
    let charging = matches!(status, 1..=3);
    let level = match soc {
        1..=100 => soc,
        _ if status == 3 || flags & 0x08 != 0 => 100,
        _ if flags & 0x04 != 0 => 50,
        _ if flags & 0x02 != 0 => 20,
        _ if flags & 0x01 != 0 => 5,
        _ => return None,
    };
    Some((level, charging))
}

/// Feature 0x1000 GetBatteryLevelStatus: level %, next level %, status.
pub fn parse_battery_status(p: &[u8]) -> Option<(u8, bool)> {
    let &[level, _next, status, ..] = p else { return None };
    match status {
        // Invalid battery or thermal error.
        5 | 6 => None,
        3 => Some((100, true)),
        _ if level == 0 || level > 100 => None,
        _ => Some((level, matches!(status, 1 | 2 | 4))),
    }
}

/// Feature 0x1001 GetBatteryVoltage: millivolts (big-endian), flags with
/// bit 7 = charging. The percentage is estimated from a Li-ion curve.
pub fn parse_battery_voltage(p: &[u8]) -> Option<(u8, bool)> {
    let &[hi, lo, flags, ..] = p else { return None };
    let mv = u16::from_be_bytes([hi, lo]);
    (mv > 0).then(|| (percent_from_millivolts(mv), flags & 0x80 != 0))
}

/// Typical single-cell Li-ion discharge curve (mV, %), interpolated linearly.
const LIION_CURVE: [(u16, u8); 11] = [
    (4150, 100),
    (4050, 90),
    (3970, 80),
    (3900, 70),
    (3840, 60),
    (3800, 50),
    (3770, 40),
    (3740, 30),
    (3700, 20),
    (3650, 10),
    (3500, 0),
];

pub fn percent_from_millivolts(mv: u16) -> u8 {
    if mv >= LIION_CURVE[0].0 {
        return 100;
    }
    for w in LIION_CURVE.windows(2) {
        let ((hi_mv, hi_pct), (lo_mv, lo_pct)) = (w[0], w[1]);
        if mv >= lo_mv {
            let span = (hi_mv - lo_mv) as u32;
            let above = (mv - lo_mv) as u32;
            return lo_pct + ((hi_pct - lo_pct) as u32 * above / span) as u8;
        }
    }
    0
}

/// Device type from feature 0x0005 GetDeviceType.
pub fn kind_from_type(t: u8) -> Option<DeviceKind> {
    match t {
        0 | 2 => Some(DeviceKind::Keyboard),
        3..=5 => Some(DeviceKind::Mouse),
        8 => Some(DeviceKind::Headset),
        _ => None,
    }
}

/// Usage page of the HID++ collections.
pub fn is_hidpp_collection(usage_page: u16, usage: u16) -> bool {
    (usage_page == 0xff00 && matches!(usage, 1 | 2)) || (usage_page == 0xff43 && usage == 0x0202)
}

#[cfg(windows)]
pub use win::poll;

#[cfg(windows)]
mod win {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use super::{query, ping, Info, DIRECT, LONG, RECEIVER_SLOTS, SHORT};
    use crate::model::{Device, DeviceKind};
    use crate::providers::hid::{self, interface_key, read_any, Channel, HidDevice, HidInfo};

    /// The short and long collections of one interface, used as one channel.
    struct HidppChannel(Vec<HidDevice>);

    impl Channel for HidppChannel {
        fn write(&mut self, report: &[u8]) -> bool {
            let want = match report.first() {
                Some(&SHORT) => 7,
                Some(&LONG) => 20,
                _ => return false,
            };
            match self.0.iter_mut().find(|d| d.info.output_len == want) {
                Some(d) => d.write(report),
                // Some devices only expose the long collection.
                None if want == 7 => {
                    let mut long = report.to_vec();
                    long[0] = LONG;
                    long.resize(20, 0);
                    self.0.iter_mut().any(|d| d.info.output_len == 20 && d.write(&long))
                }
                None => false,
            }
        }

        fn read(&mut self, timeout: Duration) -> Option<Vec<u8>> {
            read_any(&mut self.0, timeout)
        }
    }

    pub fn poll(collections: &[HidInfo]) -> Vec<Device> {
        let mut interfaces: BTreeMap<String, Vec<&HidInfo>> = BTreeMap::new();
        for c in collections.iter().filter(|c| c.vid == hid::LOGITECH && super::is_hidpp_collection(c.usage_page, c.usage)) {
            interfaces.entry(interface_key(&c.path)).or_default().push(c);
        }
        let mut devices = Vec::new();
        for (key, infos) in interfaces {
            let opened: Vec<HidDevice> = infos.iter().filter_map(|i| HidDevice::open(i)).collect();
            if opened.is_empty() {
                continue;
            }
            let product = infos.iter().find_map(|i| i.product.clone());
            let mut ch = HidppChannel(opened);
            // A device answers HID++ 2.0 at the direct index; a receiver doesn't.
            let slots: Vec<u8> = if ping(&mut ch, DIRECT).is_some() { vec![DIRECT] } else { RECEIVER_SLOTS.collect() };
            for slot in slots {
                if let Some(info) = query(&mut ch, slot) {
                    devices.push(to_device(&key, slot, info, product.as_deref()));
                }
            }
        }
        devices
    }

    fn to_device(key: &str, slot: u8, info: Info, product: Option<&str>) -> Device {
        let name = info.name.or_else(|| product.map(str::to_string)).unwrap_or_else(|| "Logitech device".into());
        Device {
            id: format!("logi:{key}:{slot}"),
            kind: info.kind.unwrap_or_else(|| DeviceKind::from_name(&name)),
            name,
            level: Some(info.level),
            charging: info.charging,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::hid::MockChannel;

    /// A fake receiver with one mouse in slot 1 that has the given features.
    fn receiver(features: &'static [(u16, u8)], battery: Vec<u8>) -> impl FnMut(&[u8]) -> Vec<Vec<u8>> {
        move |req: &[u8]| {
            let (dev, feat, fsw) = (req[1], req[2], req[3]);
            let long = |params: &[u8]| {
                let mut r = vec![LONG, dev, feat, fsw];
                r.extend_from_slice(params);
                r.resize(20, 0);
                r
            };
            let err10 = |code| vec![SHORT, dev, 0x8f, feat, fsw, code, 0];
            if dev != 1 {
                return vec![err10(0x09)];
            }
            // Unrelated traffic first: a G Hub reply and a mouse notification.
            let mut out = vec![vec![LONG, 1, feat, (fsw & 0xf0) | 0x01, 0, 0], vec![SHORT, 1, 0x04, 0, 0, 0, 0]];
            out.push(match (feat, fsw >> 4) {
                (0, 1) => long(&[4, 2, req[6]]),
                (0, 0) => {
                    let id = u16::from_be_bytes([req[4], req[5]]);
                    long(&[features.iter().find(|f| f.0 == id).map_or(0, |f| f.1)])
                }
                (2, _) => long(&battery),
                (3, 0) => long(&[10]),
                (3, 1) => long(&b"G Pro X Superlight"[req[4] as usize..]),
                (3, 2) => long(&[3]),
                _ => vec![LONG, dev, 0xff, feat, fsw, 0x05, 0],
            });
            out
        }
    }

    #[test]
    fn builds_short_requests_with_our_software_id() {
        assert_eq!(request_report(1, 0, 0, &[0x10, 0x04]), [0x10, 1, 0, 0x0b, 0x10, 0x04, 0]);
        assert_eq!(request_report(DIRECT, 5, 2, &[]), [0x10, 0xff, 5, 0x2b, 0, 0, 0]);
    }

    #[test]
    fn ignores_replies_for_other_software() {
        let r = [LONG, 1, 2, 0x11, 50, 0, 0];
        assert_eq!(parse_reply(&r, 1, 2, 1), None);
        assert_eq!(parse_reply(&[LONG, 1, 2, 0x1b, 50, 0, 0], 1, 2, 1), Some(Reply::Ok(vec![50, 0, 0])));
    }

    #[test]
    fn parses_both_error_formats() {
        assert_eq!(parse_reply(&[SHORT, 2, 0x8f, 0, 0x1b, 0x09, 0], 2, 0, 1), Some(Reply::Err(0x09)));
        assert_eq!(parse_reply(&[LONG, 1, 0xff, 3, 0x0b, 0x05, 0], 1, 3, 0), Some(Reply::Err(0x05)));
    }

    #[test]
    fn queries_a_mouse_behind_a_receiver() {
        let mut ch = MockChannel::new(receiver(&[(0x0005, 3), (0x1004, 2)], vec![73, 0x04, 0, 0]));
        assert_eq!(ping(&mut ch, DIRECT), None);
        assert_eq!(query(&mut ch, 2), None);
        let info = query(&mut ch, 1).unwrap();
        assert_eq!(info.name.as_deref(), Some("G Pro X Su"));
        assert_eq!(info.kind, Some(DeviceKind::Mouse));
        assert_eq!((info.level, info.charging), (73, false));
    }

    #[test]
    fn falls_back_to_older_battery_features() {
        let mut ch = MockChannel::new(receiver(&[(0x1000, 2)], vec![40, 20, 1]));
        let info = query(&mut ch, 1).unwrap();
        assert_eq!((info.level, info.charging, info.name), (40, true, None));

        let mut ch = MockChannel::new(receiver(&[(0x1001, 2)], vec![0x0e, 0xd8, 0x00]));
        assert_eq!(query(&mut ch, 1).unwrap().level, 50); // 3800 mV
    }

    #[test]
    fn device_without_battery_feature_is_skipped() {
        let mut ch = MockChannel::new(receiver(&[(0x0005, 3)], vec![]));
        assert_eq!(query(&mut ch, 1), None);
    }

    #[test]
    fn unified_battery_levels() {
        assert_eq!(parse_unified_battery(&[64, 0x04, 0, 0]), Some((64, false)));
        assert_eq!(parse_unified_battery(&[0, 0x02, 1, 1]), Some((20, true)));
        assert_eq!(parse_unified_battery(&[0, 0, 3, 1]), Some((100, true)));
        assert_eq!(parse_unified_battery(&[0, 0, 0, 0]), None);
    }

    #[test]
    fn battery_status_levels() {
        assert_eq!(parse_battery_status(&[90, 50, 0]), Some((90, false)));
        assert_eq!(parse_battery_status(&[0, 0, 3]), Some((100, true)));
        assert_eq!(parse_battery_status(&[30, 0, 6]), None);
        assert_eq!(parse_battery_status(&[0, 0, 0]), None);
    }

    #[test]
    fn voltage_curve() {
        assert_eq!(percent_from_millivolts(4200), 100);
        assert_eq!(percent_from_millivolts(3800), 50);
        assert_eq!(percent_from_millivolts(3720), 25);
        assert_eq!(percent_from_millivolts(3400), 0);
        assert_eq!(parse_battery_voltage(&[0x0f, 0xa0, 0x80]), Some((83, true))); // 4000 mV
    }
}
