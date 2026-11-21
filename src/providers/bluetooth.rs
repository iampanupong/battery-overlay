//! Bluetooth battery levels as Windows publishes them for the Settings app.
//!
//! Windows reads the level itself (GATT Battery Service for BLE, the HFP
//! battery indicator for classic headsets) and stores it as a device-node
//! property, so we only query Configuration Manager: no radio traffic and no
//! connections of our own. One physical device usually has several device
//! nodes (one per profile/service); they share a container ID and are merged
//! here into a single `Device`.

use std::collections::BTreeMap;

use crate::model::{Device, DeviceKind};

/// What one device node reports. Every field is optional because nodes
/// differ in which properties they carry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub instance_id: String,
    pub container_id: Option<String>,
    pub name: Option<String>,
    pub battery: Option<u8>,
    pub connected: Option<bool>,
    pub class_of_device: Option<u32>,
}

/// Merges device nodes into one `Device` per physical device, keeping only
/// connected devices that report a battery level. Sorted by name.
pub fn merge(nodes: &[Node]) -> Vec<Device> {
    let mut groups: BTreeMap<&str, Vec<&Node>> = BTreeMap::new();
    for n in nodes {
        let key = n.container_id.as_deref().unwrap_or(&n.instance_id);
        groups.entry(key).or_default().push(n);
    }

    let mut devices: Vec<Device> = groups
        .into_iter()
        .filter_map(|(key, group)| {
            let level = group.iter().find_map(|n| n.battery)?.min(100);
            // Battery values are kept after a device disconnects, so a stale
            // level must not be shown. Nodes without the property don't vote.
            let mut votes = group.iter().filter_map(|n| n.connected).peekable();
            if votes.peek().is_some() && !votes.any(|c| c) {
                return None;
            }
            let name = device_name(&group)?;
            let kind = group
                .iter()
                .find_map(|n| n.class_of_device.and_then(kind_from_class_of_device))
                .unwrap_or_else(|| kind_from_name(&name));
            Some(Device { id: format!("bt:{key}"), name, kind, level: Some(level), charging: false })
        })
        .collect();
    devices.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
    devices
}

/// The device-level node (`BTHENUM\DEV_…` / `BTHLE\DEV_…`) carries the name
/// the user gave it; service nodes have names like "… Hands-Free AG".
fn device_name(group: &[&Node]) -> Option<String> {
    let named = || group.iter().filter_map(|n| Some((n, n.name.as_deref()?.trim())).filter(|(_, s)| !s.is_empty()));
    named()
        .find(|(n, _)| n.instance_id.to_ascii_uppercase().contains("\\DEV_"))
        .or_else(|| named().min_by_key(|(_, s)| s.len()))
        .map(|(_, s)| s.to_string())
}

/// Classic Bluetooth Class of Device: major class in bits 8–12, and for
/// peripherals the keyboard/pointing bits 6–7 and the gamepad minor bits 2–5.
pub fn kind_from_class_of_device(cod: u32) -> Option<DeviceKind> {
    match (cod >> 8) & 0x1f {
        0x04 => Some(DeviceKind::Headset),
        0x05 => Some(match (cod >> 6) & 0x3 {
            0b01 => DeviceKind::Keyboard,
            0b10 | 0b11 => DeviceKind::Mouse, // combo devices count as pointing
            _ if matches!((cod >> 2) & 0xf, 0x1 | 0x2) => DeviceKind::Controller,
            _ => DeviceKind::Other,
        }),
        _ => None,
    }
}

/// Fallback for BLE devices, which have no Class of Device.
pub fn kind_from_name(name: &str) -> DeviceKind {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["mouse", "mx master", "mx anywhere", "trackball", "trackpad"]) {
        DeviceKind::Mouse
    } else if has(&["keyboard", "keys", "keychron", "mx mechanical"]) {
        DeviceKind::Keyboard
    } else if has(&["controller", "gamepad", "dualsense", "dualshock", "joy-con", "8bitdo"]) {
        DeviceKind::Controller
    } else if has(&["headset", "headphone", "buds", "airpods", "earbuds", "wh-", "wf-", "arctis", "nova", "jabra"]) {
        DeviceKind::Headset
    } else {
        DeviceKind::Other
    }
}

#[cfg(windows)]
pub use win::{all_properties, nodes, poll};

#[cfg(windows)]
mod win {
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_PropertyW, CM_Get_DevNode_Property_Keys, CM_Get_Device_ID_ListW,
        CM_Get_Device_ID_List_SizeW,
        CM_Locate_DevNodeW, CM_GETIDLIST_FILTER_ENUMERATOR, CM_LOCATE_DEVNODE_NORMAL,
        CR_BUFFER_SMALL, CR_SUCCESS,
    };
    use windows::Win32::Devices::Properties::{
        DEVPKEY_Device_ContainerId, DEVPKEY_Device_FriendlyName, DEVPROPKEY, DEVPROPTYPE,
        DEVPROP_TYPE_BOOLEAN, DEVPROP_TYPE_BYTE, DEVPROP_TYPE_GUID, DEVPROP_TYPE_STRING,
        DEVPROP_TYPE_UINT32,
    };

    use super::Node;
    use crate::model::Device;

    /// Battery percentage shown in Settings › Bluetooth & devices (undocumented).
    const BATTERY_LEVEL: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
        pid: 2,
    };
    /// Whether the device is currently connected (undocumented).
    const IS_CONNECTED: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0x83da6326_97a6_4088_9453_a1923f573b29),
        pid: 15,
    };
    /// DEVPKEY_Bluetooth_ClassOfDevice (classic devices only).
    const CLASS_OF_DEVICE: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0x2bd67d8b_8beb_48d5_87e0_6cda3428040a),
        pid: 10,
    };

    pub fn poll() -> Vec<Device> {
        super::merge(&nodes())
    }

    /// Every present classic and LE Bluetooth device node with its raw properties.
    pub fn nodes() -> Vec<Node> {
        ["BTHENUM", "BTHLE"]
            .iter()
            .flat_map(|e| device_ids(e))
            .filter_map(|id| read_node(&id))
            .collect()
    }

    fn device_ids(enumerator: &str) -> Vec<String> {
        let filter = wide(enumerator);
        let filter = PCWSTR(filter.as_ptr());
        // The list can grow between the size query and the fetch; retry then.
        for _ in 0..4 {
            let mut len = 0u32;
            if unsafe { CM_Get_Device_ID_List_SizeW(&mut len, filter, CM_GETIDLIST_FILTER_ENUMERATOR) } != CR_SUCCESS {
                return Vec::new();
            }
            let mut buf = vec![0u16; len as usize];
            match unsafe { CM_Get_Device_ID_ListW(filter, &mut buf, CM_GETIDLIST_FILTER_ENUMERATOR) } {
                CR_SUCCESS => {
                    return buf
                        .split(|&c| c == 0)
                        .filter(|s| !s.is_empty())
                        .map(String::from_utf16_lossy)
                        .collect()
                }
                CR_BUFFER_SMALL => continue,
                _ => return Vec::new(),
            }
        }
        Vec::new()
    }

    /// `None` when the node isn't present (e.g. a removed device).
    fn locate(instance_id: &str) -> Option<u32> {
        let id = wide(instance_id);
        let mut devinst = 0u32;
        let r = unsafe { CM_Locate_DevNodeW(&mut devinst, PCWSTR(id.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) };
        (r == CR_SUCCESS).then_some(devinst)
    }

    fn read_node(instance_id: &str) -> Option<Node> {
        let devinst = locate(instance_id)?;
        Some(Node {
            instance_id: instance_id.to_string(),
            container_id: property(devinst, &DEVPKEY_Device_ContainerId, DEVPROP_TYPE_GUID)
                .filter(|b| b.len() == 16)
                .map(|b| b.iter().map(|x| format!("{x:02x}")).collect()),
            name: property(devinst, &DEVPKEY_Device_FriendlyName, DEVPROP_TYPE_STRING)
                .map(|b| utf16_string(&b)),
            battery: property(devinst, &BATTERY_LEVEL, DEVPROP_TYPE_BYTE).and_then(|b| b.first().copied()),
            connected: property(devinst, &IS_CONNECTED, DEVPROP_TYPE_BOOLEAN)
                .and_then(|b| b.first().map(|&v| v != 0)),
            class_of_device: property(devinst, &CLASS_OF_DEVICE, DEVPROP_TYPE_UINT32)
                .filter(|b| b.len() == 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        })
    }

    /// Every property the node carries as `("{fmtid} pid", type, bytes)`, for `--probe --all`.
    pub fn all_properties(instance_id: &str) -> Vec<(String, u32, Vec<u8>)> {
        let Some(devinst) = locate(instance_id) else { return Vec::new() };
        let mut count = 0u32;
        if unsafe { CM_Get_DevNode_Property_Keys(devinst, None, &mut count, 0) } != CR_BUFFER_SMALL {
            return Vec::new();
        }
        let mut keys = vec![DEVPROPKEY::default(); count as usize];
        if unsafe { CM_Get_DevNode_Property_Keys(devinst, Some(keys.as_mut_ptr()), &mut count, 0) } != CR_SUCCESS {
            return Vec::new();
        }
        keys.truncate(count as usize);
        keys.iter()
            .filter_map(|k| {
                let (ty, data) = raw_property(devinst, k)?;
                Some((format!("{{{:?}}} {}", k.fmtid, k.pid), ty.0, data))
            })
            .collect()
    }

    /// Raw property bytes, or `None` if it's missing or of an unexpected type.
    fn property(devinst: u32, key: &DEVPROPKEY, expected: DEVPROPTYPE) -> Option<Vec<u8>> {
        raw_property(devinst, key).and_then(|(ty, data)| (ty == expected && !data.is_empty()).then_some(data))
    }

    fn raw_property(devinst: u32, key: &DEVPROPKEY) -> Option<(DEVPROPTYPE, Vec<u8>)> {
        let mut ty = DEVPROPTYPE(0);
        let mut size = 0u32;
        let r = unsafe { CM_Get_DevNode_PropertyW(devinst, key, &mut ty, None, &mut size, 0) };
        if r != CR_BUFFER_SMALL {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let r = unsafe { CM_Get_DevNode_PropertyW(devinst, key, &mut ty, Some(buf.as_mut_ptr()), &mut size, 0) };
        (r == CR_SUCCESS).then(|| {
            buf.truncate(size as usize);
            (ty, buf)
        })
    }

    fn utf16_string(b: &[u8]) -> String {
        let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
        String::from_utf16_lossy(&units).trim_end_matches('\0').to_string()
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, container: &str, name: &str) -> Node {
        Node {
            instance_id: id.into(),
            container_id: Some(container.into()),
            name: Some(name.into()),
            ..Default::default()
        }
    }

    #[test]
    fn merges_nodes_of_one_device_and_prefers_device_node_name() {
        let nodes = vec![
            Node { battery: Some(64), ..node(r"BTHENUM\{0000111E-0000}_VID&0002", "c1", "WH-1000XM4 Hands-Free AG") },
            Node { connected: Some(true), ..node(r"BTHENUM\DEV_38184C000000\7&1", "c1", "WH-1000XM4") },
        ];
        let d = merge(&nodes);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].name, "WH-1000XM4");
        assert_eq!(d[0].level, Some(64));
        assert_eq!(d[0].id, "bt:c1");
        assert_eq!(d[0].kind, DeviceKind::Headset);
    }

    #[test]
    fn hides_disconnected_devices_with_stale_levels() {
        let nodes = vec![Node { battery: Some(80), connected: Some(false), ..node(r"BTHLE\DEV_A", "c1", "MX Master 3") }];
        assert!(merge(&nodes).is_empty());
    }

    #[test]
    fn missing_connected_property_counts_as_connected() {
        let nodes = vec![Node { battery: Some(80), ..node(r"BTHLE\DEV_A", "c1", "MX Master 3") }];
        assert_eq!(merge(&nodes)[0].kind, DeviceKind::Mouse);
    }

    #[test]
    fn any_connected_node_makes_device_connected() {
        let nodes = vec![
            Node { battery: Some(50), connected: Some(false), ..node(r"BTHENUM\{1}", "c1", "Pad Svc") },
            Node { connected: Some(true), ..node(r"BTHENUM\DEV_1", "c1", "Pad") },
        ];
        assert_eq!(merge(&nodes).len(), 1);
    }

    #[test]
    fn skips_devices_without_battery_and_sorts_by_name() {
        let nodes = vec![
            Node { battery: Some(10), ..node(r"BTHLE\DEV_B", "b", "zeta keyboard") },
            node(r"BTHLE\DEV_C", "c", "No Battery Speaker"),
            Node { battery: Some(90), ..node(r"BTHLE\DEV_A", "a", "Alpha Mouse") },
        ];
        let names: Vec<_> = merge(&nodes).into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["Alpha Mouse", "zeta keyboard"]);
    }

    #[test]
    fn nodes_without_container_stay_separate_and_levels_are_clamped() {
        let nodes = vec![
            Node { container_id: None, battery: Some(250), ..node(r"BTHLE\DEV_A", "", "A") },
            Node { container_id: None, battery: Some(5), ..node(r"BTHLE\DEV_B", "", "B") },
        ];
        let d = merge(&nodes);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].level, Some(100));
    }

    #[test]
    fn class_of_device_classification() {
        assert_eq!(kind_from_class_of_device(0x240404), Some(DeviceKind::Headset)); // wearable headset
        assert_eq!(kind_from_class_of_device(0x002540), Some(DeviceKind::Keyboard));
        assert_eq!(kind_from_class_of_device(0x002580), Some(DeviceKind::Mouse));
        assert_eq!(kind_from_class_of_device(0x002508), Some(DeviceKind::Controller)); // gamepad
        assert_eq!(kind_from_class_of_device(0x00010c), None); // computer
    }

    #[test]
    fn class_of_device_beats_name() {
        let nodes = vec![Node { battery: Some(40), class_of_device: Some(0x002580), ..node(r"BTHENUM\DEV_A", "a", "Gizmo") }];
        assert_eq!(merge(&nodes)[0].kind, DeviceKind::Mouse);
    }
}
