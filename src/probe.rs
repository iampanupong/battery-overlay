//! `--probe`: prints what the providers see, for checking real devices.
//! `--probe --all` also dumps every raw property of each Bluetooth device node,
//! one per line in a stable order, so two runs (e.g. on and off the charger)
//! can be compared with `fc` or `diff`.

/// Short labels for properties we know, to make the dump easier to read.
fn known_name(key: &str) -> Option<&'static str> {
    Some(match key {
        "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2" => "battery level",
        "{83DA6326-97A6-4088-9453-A1923F573B29} 15" => "connected",
        "{83DA6326-97A6-4088-9453-A1923F573B29} 102" => "last arrival (changes on reconnect)",
        "{83DA6326-97A6-4088-9453-A1923F573B29} 103" => "last removal (changes on disconnect)",
        "{A45C254E-DF1C-4EFD-8020-67D146A850E0} 14" => "friendly name",
        "{8C7ED206-3F8A-4827-B3AB-AE9E1FAEFC6C} 2" => "container id",
        "{2BD67D8B-8BEB-48D5-87E0-6CDA3428040A} 10" => "class of device",
        _ => return None,
    })
}

/// DEVPROPTYPE codes from devpropdef.h.
fn type_name(ty: u32) -> String {
    match ty {
        0x02 => "SByte".into(),
        0x03 => "Byte".into(),
        0x04 => "Int16".into(),
        0x05 => "UInt16".into(),
        0x06 => "Int32".into(),
        0x07 => "UInt32".into(),
        0x08 => "Int64".into(),
        0x09 => "UInt64".into(),
        0x0d => "Guid".into(),
        0x10 => "FileTime".into(),
        0x11 => "Boolean".into(),
        0x12 => "String".into(),
        0x13 => "SecurityDesc".into(),
        0x15 => "PropKey".into(),
        0x17 => "Win32Error".into(),
        0x18 => "NtStatus".into(),
        0x1003 => "Binary".into(),
        0x2012 => "StringList".into(),
        _ => format!("Type{ty:#x}"),
    }
}

/// Human-readable value; falls back to hex for unknown types or odd sizes.
fn format_value(ty: u32, data: &[u8]) -> String {
    fn le<const N: usize>(d: &[u8]) -> Option<[u8; N]> {
        d.try_into().ok()
    }
    let formatted = match ty {
        0x02 => le::<1>(data).map(|b| (b[0] as i8).to_string()),
        0x03 => le::<1>(data).map(|b| b[0].to_string()),
        0x04 => le(data).map(|b| i16::from_le_bytes(b).to_string()),
        0x05 => le(data).map(|b| u16::from_le_bytes(b).to_string()),
        0x06 | 0x17 => le(data).map(|b| i32::from_le_bytes(b).to_string()),
        0x07 | 0x18 => le(data).map(|b| {
            let v = u32::from_le_bytes(b);
            format!("{v} ({v:#010x})")
        }),
        0x08 => le(data).map(|b| i64::from_le_bytes(b).to_string()),
        0x09 | 0x10 => le(data).map(|b| u64::from_le_bytes(b).to_string()),
        0x0d => le::<16>(data).map(|b| format_guid(&b)),
        0x11 => le::<1>(data).map(|b| (b[0] != 0).to_string()),
        0x12 => Some(format!("{:?}", utf16(data).trim_end_matches('\0'))),
        0x2012 => Some(
            utf16(data)
                .split('\0')
                .filter(|s| !s.is_empty())
                .map(|s| format!("{s:?}"))
                .collect::<Vec<_>>()
                .join(" | "),
        ),
        0x15 => (data.len() == 20).then(|| {
            let pid = u32::from_le_bytes([data[16], data[17], data[18], data[19]]);
            format!("{{{}}} {pid}", format_guid(&data[..16].try_into().unwrap()))
        }),
        _ => None,
    };
    formatted.unwrap_or_else(|| hex(data))
}

/// Windows GUID layout: three little-endian fields, then 8 raw bytes.
fn format_guid(b: &[u8; 16]) -> String {
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{}",
        u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        u16::from_le_bytes([b[4], b[5]]),
        u16::from_le_bytes([b[6], b[7]]),
        b[8],
        b[9],
        b[10..].iter().map(|x| format!("{x:02X}")).collect::<String>(),
    )
}

fn utf16(data: &[u8]) -> String {
    let units: Vec<u16> = data.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
    String::from_utf16_lossy(&units)
}

fn hex(data: &[u8]) -> String {
    const MAX: usize = 64;
    let shown: Vec<String> = data.iter().take(MAX).map(|b| format!("{b:02x}")).collect();
    let mut s = shown.join(" ");
    if data.len() > MAX {
        s.push_str(&format!(" … ({} bytes)", data.len()));
    }
    s
}

/// One line of the `--all` dump.
fn property_line(key: &str, ty: u32, data: &[u8]) -> String {
    let label = known_name(key).map(|n| format!("  [{n}]")).unwrap_or_default();
    format!("    {key:<44} {:<12} {}{label}", type_name(ty), format_value(ty, data))
}

#[cfg(windows)]
pub fn run(all: bool) {
    use crate::providers::{self, bluetooth};
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // Release builds have no console of their own; borrow the launching one.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };

    let mut nodes = bluetooth::nodes();
    nodes.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
    println!("Bluetooth device nodes:");
    for n in &nodes {
        println!(
            "  {}\n    name={:?} container={:?} battery={:?} connected={:?} cod={:?}",
            n.instance_id,
            n.name,
            n.container_id,
            n.battery,
            n.connected,
            n.class_of_device.map(|c| format!("{c:#08x}")),
        );
        if all {
            let mut props = bluetooth::all_properties(&n.instance_id);
            props.sort_by(|a, b| a.0.cmp(&b.0));
            for (key, ty, data) in &props {
                println!("{}", property_line(key, *ty, data));
            }
            println!();
        }
    }
    println!("\nDevices shown in the overlay:");
    for d in providers::poll_all() {
        println!("  {:?} {:?} level={:?} id={}", d.kind, d.name, d.level, d.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16_bytes(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn formats_scalars() {
        assert_eq!(format_value(0x03, &[64]), "64");
        assert_eq!(format_value(0x11, &[0xff]), "true");
        assert_eq!(format_value(0x11, &[0]), "false");
        assert_eq!(format_value(0x07, &0x2580u32.to_le_bytes()), "9600 (0x00002580)");
        assert_eq!(format_value(0x04, &(-2i16).to_le_bytes()), "-2");
    }

    #[test]
    fn formats_strings_and_lists() {
        assert_eq!(format_value(0x12, &utf16_bytes("MX Master 3\0")), "\"MX Master 3\"");
        assert_eq!(format_value(0x2012, &utf16_bytes("a\0b\0\0")), "\"a\" | \"b\"");
    }

    #[test]
    fn formats_guid_with_windows_byte_order() {
        let b = [
            0x19, 0xa3, 0x4e, 0x10, 0xe2, 0x6e, 0x01, 0x47, 0xbd, 0x47, 0x8d, 0xdb, 0xf4, 0x25, 0xbb, 0xe5,
        ];
        assert_eq!(format_value(0x0d, &b), "104EA319-6EE2-4701-BD47-8DDBF425BBE5");
    }

    #[test]
    fn falls_back_to_hex_for_unknown_types_and_bad_sizes() {
        assert_eq!(format_value(0x1003, &[0xde, 0xad]), "de ad");
        assert_eq!(format_value(0x07, &[1, 2]), "01 02");
        assert!(format_value(0x1003, &[0; 100]).ends_with("… (100 bytes)"));
    }

    #[test]
    fn labels_known_keys() {
        let line = property_line("{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2", 0x03, &[42]);
        assert!(line.contains("Byte"));
        assert!(line.contains("42  [battery level]"));
        assert!(!property_line("{00000000-0000-0000-0000-000000000000} 1", 0x03, &[1]).contains('['));
    }
}
