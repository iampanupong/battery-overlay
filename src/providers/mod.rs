//! Battery sources. Each provider's parsing/merging logic is platform-independent;
//! only the OS queries are Windows-only.

pub mod bluetooth;
pub mod corsair;
pub mod hid;
pub mod logitech;
pub mod razer;
pub mod steelseries;
pub mod xbox;

#[cfg(windows)]
use crate::model::Device;

/// Reads every provider once. Vendor HID queries can wait on a sleeping
/// device for a few hundred milliseconds, so call this off the UI thread.
#[cfg(windows)]
pub fn poll_all() -> Vec<Device> {
    let mut devices = bluetooth::poll();
    let xinput = xbox::without_bluetooth_duplicates(&devices, xbox::poll());
    devices.extend(xinput);
    let collections = hid::enumerate(&hid::VENDORS);
    devices.extend(logitech::poll(&collections));
    devices.extend(razer::poll(&collections));
    devices.extend(steelseries::poll(&collections));
    devices.extend(corsair::poll(&collections));
    sort(&mut devices);
    devices
}

/// By name, then ID, so the HUD order is stable between polls.
pub fn sort(devices: &mut [crate::model::Device]) {
    devices.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
}
