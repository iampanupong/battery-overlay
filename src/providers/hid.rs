//! Shared HID plumbing for the vendor providers (Logitech, Razer, SteelSeries,
//! Corsair).
//!
//! Only USB devices from the vendors we know are ever opened (`--probe --all`
//! also describes other vendors' devices, with zero access); Bluetooth HID
//! devices are skipped because the Bluetooth provider already reports them.
//! Handles are always opened shared, so vendor software (G Hub, Synapse,
//! GG, iCUE) keeps working alongside us. Every input report is delivered to
//! every open handle, so replies are matched by content, never assumed to be
//! the next report.

use std::time::{Duration, Instant};

pub const LOGITECH: u16 = 0x046d;
pub const RAZER: u16 = 0x1532;
pub const STEELSERIES: u16 = 0x1038;
pub const CORSAIR: u16 = 0x1b1c;
pub const VENDORS: [u16; 4] = [LOGITECH, RAZER, STEELSERIES, CORSAIR];

/// One HID top-level collection. Windows exposes each collection of a USB
/// interface as its own device interface with its own path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HidInfo {
    pub path: String,
    pub vid: u16,
    pub pid: u16,
    /// USB interface number (`mi_XX` in the path); `None` for single-interface devices.
    pub interface: Option<u8>,
    pub usage_page: u16,
    pub usage: u16,
    /// Report lengths including the report ID byte; 0 when the collection has none.
    pub input_len: u16,
    pub output_len: u16,
    pub feature_len: u16,
    pub product: Option<String>,
}

/// Vendor ID, product ID and interface number from a USB HID interface path
/// such as `\\?\hid#vid_046d&pid_c539&mi_02&col01#8&2a3b&0&0001#{4d1e55b2-…}`.
/// Bluetooth HID paths (`hid#{00001124-…}_vid&…`) don't match and give `None`.
pub fn parse_usb_path(path: &str) -> Option<(u16, u16, Option<u8>)> {
    let ids = path.split('#').nth(1)?.to_ascii_lowercase();
    let field = |name: &str| ids.split('&').find_map(|p| p.strip_prefix(name));
    let vid = u16::from_str_radix(field("vid_")?, 16).ok()?;
    let pid = u16::from_str_radix(field("pid_")?, 16).ok()?;
    let interface = field("mi_").and_then(|m| u8::from_str_radix(m, 16).ok());
    Some((vid, pid, interface))
}

/// A key shared by all collections of one USB interface: the path without
/// the collection number and without the per-collection instance suffix.
pub fn interface_key(path: &str) -> String {
    let mut parts = path.split('#');
    let _prefix = parts.next();
    let ids = parts.next().unwrap_or_default();
    let instance = parts.next().unwrap_or_default();
    let ids: Vec<&str> = ids.split('&').filter(|p| !p.to_ascii_lowercase().starts_with("col")).collect();
    let instance = instance.rsplit_once('&').map_or(instance, |(head, _)| head);
    format!("{}#{}", ids.join("&"), instance).to_ascii_lowercase()
}

/// Strips receiver/dongle suffixes from a product string so the HUD shows the
/// device's own name, e.g. "Razer Viper Ultimate Dongle" → "Razer Viper Ultimate".
pub fn clean_product_name(name: &str) -> String {
    let mut n = name.trim();
    for suffix in [" Dongle", " Receiver", " Wireless Receiver", " USB Receiver"] {
        if let Some(head) = n.strip_suffix(suffix) {
            n = head.trim_end();
        }
    }
    n.to_string()
}

/// A vendor-defined collection of a vendor we don't support yet: what
/// `--probe --all` lists to find the dongles of new vendors.
pub fn is_unknown_vendor_defined(c: &HidInfo) -> bool {
    !VENDORS.contains(&c.vid) && c.usage_page >= 0xff00
}

/// Output/input report exchange with one device (possibly over several
/// collections of the same interface).
pub trait Channel {
    /// Sends one output report. `report[0]` is the report ID (0 if unnumbered).
    fn write(&mut self, report: &[u8]) -> bool;
    /// Next input report (report ID first), or `None` after `timeout`.
    fn read(&mut self, timeout: Duration) -> Option<Vec<u8>>;
}

/// Sends `request` and returns the first input report that `accept` maps to
/// `Some`, skipping unrelated traffic, or `None` after `timeout`.
pub fn transact<T>(
    ch: &mut dyn Channel,
    request: &[u8],
    timeout: Duration,
    mut accept: impl FnMut(&[u8]) -> Option<T>,
) -> Option<T> {
    if !ch.write(request) {
        return None;
    }
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        if let Some(v) = accept(&ch.read(left)?) {
            return Some(v);
        }
    }
}

/// Feature-report exchange, for protocols that don't use input reports.
pub trait FeatureChannel {
    /// Sends a feature report. `report[0]` is the report ID.
    fn set_feature(&mut self, report: &[u8]) -> bool;
    /// Reads feature report `report_id` into a buffer of `len` bytes.
    fn get_feature(&mut self, report_id: u8, len: usize) -> Option<Vec<u8>>;
    /// Gives the device time to prepare a reply; a no-op in tests.
    fn pause(&mut self) {}
}

#[cfg(windows)]
pub use win::{enumerate, enumerate_where, read_any, HidDevice};

#[cfg(windows)]
mod win {
    use std::time::{Duration, Instant};

    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW, CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        CR_BUFFER_SMALL, CR_SUCCESS,
    };
    use windows::Win32::Devices::HumanInterfaceDevice::{
        HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid, HidD_GetPreparsedData,
        HidD_GetProductString, HidD_SetFeature, HidP_GetCaps, HIDD_ATTRIBUTES, HIDP_CAPS, PHIDP_PREPARSED_DATA,
    };
    use windows::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects, WaitForSingleObject};

    use super::{parse_usb_path, Channel, FeatureChannel, HidInfo};

    const HIDP_STATUS_SUCCESS: i32 = 0x0011_0000;
    const WRITE_TIMEOUT_MS: u32 = 1000;

    /// Every present USB HID collection of the given vendors. Devices of other
    /// vendors are filtered by path and never opened.
    pub fn enumerate(vendors: &[u16]) -> Vec<HidInfo> {
        enumerate_where(|vid| vendors.contains(&vid))
    }

    /// USB HID collections whose vendor ID passes `keep_vid`; others are never opened.
    pub fn enumerate_where(keep_vid: impl Fn(u16) -> bool) -> Vec<HidInfo> {
        let guid = unsafe { HidD_GetHidGuid() };
        interface_paths(&guid)
            .into_iter()
            .filter_map(|path| {
                let (vid, pid, interface) = parse_usb_path(&path)?;
                if !keep_vid(vid) {
                    return None;
                }
                // Zero access is enough for attributes and caps, and works even
                // for keyboards and mice, which Windows opens exclusively.
                let handle = open_handle(&path, 0, false)?;
                let info = describe(handle, path, vid, pid, interface);
                unsafe { let _ = CloseHandle(handle); }
                info
            })
            .collect()
    }

    fn interface_paths(guid: &GUID) -> Vec<String> {
        // The list can grow between the size query and the fetch; retry then.
        for _ in 0..4 {
            let mut len = 0u32;
            if unsafe { CM_Get_Device_Interface_List_SizeW(&mut len, guid, PCWSTR::null(), CM_GET_DEVICE_INTERFACE_LIST_PRESENT) }
                != CR_SUCCESS
            {
                return Vec::new();
            }
            let mut buf = vec![0u16; len as usize];
            match unsafe { CM_Get_Device_Interface_ListW(guid, PCWSTR::null(), &mut buf, CM_GET_DEVICE_INTERFACE_LIST_PRESENT) } {
                CR_SUCCESS => {
                    return buf.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
                }
                CR_BUFFER_SMALL => continue,
                _ => return Vec::new(),
            }
        }
        Vec::new()
    }

    fn describe(handle: HANDLE, path: String, vid: u16, pid: u16, interface: Option<u8>) -> Option<HidInfo> {
        let mut attrs = HIDD_ATTRIBUTES { Size: size_of::<HIDD_ATTRIBUTES>() as u32, ..Default::default() };
        if unsafe { HidD_GetAttributes(handle, &mut attrs) }.0 == 0 || attrs.VendorID != vid {
            return None;
        }
        let mut caps = HIDP_CAPS::default();
        let mut data = PHIDP_PREPARSED_DATA::default();
        if unsafe { HidD_GetPreparsedData(handle, &mut data) }.0 == 0 {
            return None;
        }
        let status = unsafe { HidP_GetCaps(data, &mut caps) };
        unsafe { let _ = HidD_FreePreparsedData(data); }
        if status.0 != HIDP_STATUS_SUCCESS {
            return None;
        }
        let mut name = [0u16; 128];
        let product = (unsafe { HidD_GetProductString(handle, name.as_mut_ptr().cast(), size_of_val(&name) as u32) }.0 != 0)
            .then(|| {
                let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
                String::from_utf16_lossy(&name[..end]).trim().to_string()
            })
            .filter(|s| !s.is_empty());
        Some(HidInfo {
            path,
            vid,
            pid,
            interface,
            usage_page: caps.UsagePage,
            usage: caps.Usage,
            input_len: caps.InputReportByteLength,
            output_len: caps.OutputReportByteLength,
            feature_len: caps.FeatureReportByteLength,
            product,
        })
    }

    fn open_handle(path: &str, access: u32, overlapped: bool) -> Option<HANDLE> {
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let flags = if overlapped { FILE_FLAG_OVERLAPPED } else { FILE_FLAGS_AND_ATTRIBUTES(0) };
        unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                flags,
                None,
            )
        }
        .ok()
    }

    /// An open collection. Input reports are read with overlapped I/O so a
    /// silent device can't block the poll past its timeout.
    pub struct HidDevice {
        handle: HANDLE,
        pub info: HidInfo,
        event: HANDLE,
        // Boxed so the kernel's pointers stay valid while a read is pending.
        overlapped: Box<OVERLAPPED>,
        buf: Box<[u8]>,
        pending: bool,
    }

    impl HidDevice {
        /// Opens shared for reading and writing input/output reports.
        pub fn open(info: &HidInfo) -> Option<Self> {
            Self::wrap(info, open_handle(&info.path, (GENERIC_READ | GENERIC_WRITE).0, true)?)
        }

        /// Opens for feature reports only. `HidD_*Feature` are synchronous, so
        /// the handle isn't overlapped; collections Windows holds exclusively
        /// (mice, keyboards) still allow feature reports with zero access.
        pub fn open_for_features(info: &HidInfo) -> Option<Self> {
            let handle = open_handle(&info.path, (GENERIC_READ | GENERIC_WRITE).0, false)
                .or_else(|| open_handle(&info.path, 0, false))?;
            let mut dev = Self::wrap(info, handle)?;
            // Never read input reports through a synchronous handle.
            dev.info.input_len = 0;
            Some(dev)
        }

        fn wrap(info: &HidInfo, handle: HANDLE) -> Option<Self> {
            let Ok(event) = (unsafe { CreateEventW(None, true, false, None) }) else {
                unsafe { let _ = CloseHandle(handle); }
                return None;
            };
            Some(Self {
                handle,
                info: info.clone(),
                event,
                overlapped: Box::new(OVERLAPPED { hEvent: event, ..Default::default() }),
                buf: vec![0u8; (info.input_len as usize).max(1)].into_boxed_slice(),
                pending: false,
            })
        }

        /// Starts a read if none is pending; `false` if the device can't be read.
        fn arm(&mut self) -> bool {
            if self.pending {
                return true;
            }
            if self.info.input_len == 0 {
                return false;
            }
            let r = unsafe { ReadFile(self.handle, Some(&mut self.buf), None, Some(&mut *self.overlapped)) };
            // Completing immediately also signals the event, so both cases wait the same way.
            self.pending = r.is_ok() || r.is_err_and(|e| e.code() == ERROR_IO_PENDING.to_hresult());
            self.pending
        }

        fn finish(&mut self) -> Option<Vec<u8>> {
            self.pending = false;
            let mut n = 0u32;
            unsafe { GetOverlappedResult(self.handle, &*self.overlapped, &mut n, false) }.ok()?;
            Some(self.buf[..n as usize].to_vec())
        }
    }

    /// The next input report from any of `devices`, or `None` after `timeout`.
    pub fn read_any(devices: &mut [HidDevice], timeout: Duration) -> Option<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        loop {
            let armed: Vec<usize> = (0..devices.len()).filter(|&i| devices[i].arm()).collect();
            let events: Vec<HANDLE> = armed.iter().map(|&i| devices[i].event).collect();
            if events.is_empty() {
                return None;
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            let r = unsafe { WaitForMultipleObjects(&events, false, left.as_millis() as u32) };
            let hit = r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
            let &i = armed.get(hit)?;
            // A failed read (e.g. the device was unplugged) is skipped; the
            // next loop re-arms it or gives up when the deadline passes.
            if let Some(report) = devices[i].finish() {
                return Some(report);
            }
        }
    }

    impl Channel for HidDevice {
        fn write(&mut self, report: &[u8]) -> bool {
            write_report(self.handle, self.info.output_len as usize, report)
        }

        fn read(&mut self, timeout: Duration) -> Option<Vec<u8>> {
            read_any(std::slice::from_mut(self), timeout)
        }
    }

    impl FeatureChannel for HidDevice {
        fn set_feature(&mut self, report: &[u8]) -> bool {
            let mut buf = report.to_vec();
            buf.resize((self.info.feature_len as usize).max(report.len()), 0);
            unsafe { HidD_SetFeature(self.handle, buf.as_ptr().cast(), buf.len() as u32) }.0 != 0
        }

        fn get_feature(&mut self, report_id: u8, len: usize) -> Option<Vec<u8>> {
            let mut buf = vec![0u8; len.max(self.info.feature_len as usize)];
            buf[0] = report_id;
            (unsafe { HidD_GetFeature(self.handle, buf.as_mut_ptr().cast(), buf.len() as u32) }.0 != 0).then_some(buf)
        }

        fn pause(&mut self) {
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    /// Windows requires output reports padded to the collection's report length.
    pub(super) fn write_report(handle: HANDLE, len: usize, report: &[u8]) -> bool {
        if len == 0 || report.len() > len {
            return false;
        }
        let mut buf = report.to_vec();
        buf.resize(len, 0);
        let Ok(event) = (unsafe { CreateEventW(None, true, false, None) }) else { return false };
        let mut ov = OVERLAPPED { hEvent: event, ..Default::default() };
        let ok = unsafe {
            match WriteFile(handle, Some(&buf), None, Some(&mut ov)) {
                Ok(()) => true,
                Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                    if WaitForSingleObject(event, WRITE_TIMEOUT_MS) != WAIT_OBJECT_0 {
                        let _ = CancelIoEx(handle, Some(&ov));
                    }
                    // Waits for the cancel to land so `buf` and `ov` outlive the I/O.
                    let mut n = 0u32;
                    GetOverlappedResult(handle, &ov, &mut n, true).is_ok()
                }
                Err(_) => false,
            }
        };
        unsafe { let _ = CloseHandle(event); }
        ok
    }

    impl Drop for HidDevice {
        fn drop(&mut self) {
            unsafe {
                if self.pending {
                    let _ = CancelIoEx(self.handle, Some(&*self.overlapped));
                    let mut n = 0u32;
                    let _ = GetOverlappedResult(self.handle, &*self.overlapped, &mut n, true);
                }
                let _ = CloseHandle(self.event);
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

/// Scripted device for protocol tests: answers each written report with the
/// reports `respond` returns for it.
#[cfg(test)]
pub struct MockChannel<F: FnMut(&[u8]) -> Vec<Vec<u8>>> {
    pub respond: F,
    pub queue: std::collections::VecDeque<Vec<u8>>,
    pub written: Vec<Vec<u8>>,
}

#[cfg(test)]
impl<F: FnMut(&[u8]) -> Vec<Vec<u8>>> MockChannel<F> {
    pub fn new(respond: F) -> Self {
        Self { respond, queue: Default::default(), written: Vec::new() }
    }
}

#[cfg(test)]
impl<F: FnMut(&[u8]) -> Vec<Vec<u8>>> Channel for MockChannel<F> {
    fn write(&mut self, report: &[u8]) -> bool {
        self.written.push(report.to_vec());
        let replies = (self.respond)(report);
        self.queue.extend(replies);
        true
    }

    fn read(&mut self, _timeout: Duration) -> Option<Vec<u8>> {
        self.queue.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_unknown_vendor_defined_collections() {
        let c = |vid, usage_page| HidInfo { vid, usage_page, ..Default::default() };
        assert!(is_unknown_vendor_defined(&c(0x3554, 0xff00)));
        assert!(is_unknown_vendor_defined(&c(0x3554, 0xffff)));
        assert!(!is_unknown_vendor_defined(&c(0x3554, 0xfeff)));
        assert!(!is_unknown_vendor_defined(&c(0x3554, 0x0001)));
        assert!(!is_unknown_vendor_defined(&c(0x3554, 0x000c)));
        assert!(!is_unknown_vendor_defined(&c(LOGITECH, 0xff00)));
    }

    #[test]
    fn parses_usb_paths() {
        let p = r"\\?\hid#vid_046d&pid_c539&mi_02&col01#8&2a3b4c5d&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(parse_usb_path(p), Some((0x046d, 0xc539, Some(2))));
        let p = r"\\?\HID#VID_1532&PID_007A#7&1a2b&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(parse_usb_path(p), Some((0x1532, 0x007a, None)));
    }

    #[test]
    fn bluetooth_paths_are_not_usb() {
        let classic = r"\\?\hid#{00001124-0000-1000-8000-00805f9b34fb}_vid&0002046d_pid&b023&col01#9&1&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let le = r"\\?\hid#{00001812-0000-1000-8000-00805f9b34fb}_dev_vid&02046d_pid&b023_rev&0005#9&1&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(parse_usb_path(classic), None);
        assert_eq!(parse_usb_path(le), None);
    }

    #[test]
    fn collections_of_one_interface_share_a_key() {
        let a = r"\\?\hid#vid_046d&pid_c539&mi_02&col01#8&2a3b4c5d&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let b = r"\\?\HID#VID_046D&PID_C539&MI_02&Col02#8&2a3b4c5d&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let other = r"\\?\hid#vid_046d&pid_c539&mi_01#8&99&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(interface_key(a), interface_key(b));
        assert_ne!(interface_key(a), interface_key(other));
    }

    #[test]
    fn cleans_receiver_suffixes() {
        assert_eq!(clean_product_name("Razer Viper Ultimate Dongle"), "Razer Viper Ultimate");
        assert_eq!(clean_product_name(" Arctis Nova 7 "), "Arctis Nova 7");
    }

    #[test]
    fn transact_skips_unrelated_reports() {
        let mut ch = MockChannel::new(|_| vec![vec![1, 2], vec![9, 9], vec![1, 3]]);
        assert_eq!(transact(&mut ch, &[0], Duration::from_secs(1), |r| (r[0] == 9).then_some(r[1])), Some(9));
        assert_eq!(transact(&mut ch, &[0], Duration::from_secs(1), |r| (r[0] == 7).then_some(())), None);
    }
}
