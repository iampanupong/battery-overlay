// Values match the Win32 MOD_* / VK_* constants so parsing stays testable off Windows.
pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
pub const MOD_NOREPEAT: u32 = 0x4000;
const VK_F1: u32 = 0x70;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub vk: u32,
}

/// Parses strings like "Ctrl+Shift+B" or "Alt+F9". At least one modifier is
/// required so the hotkey can't swallow a plain key the game needs.
pub fn parse(s: &str) -> Option<Hotkey> {
    let mut modifiers = 0;
    let mut vk = None;
    for part in s.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= MOD_CONTROL,
            "shift" => modifiers |= MOD_SHIFT,
            "alt" => modifiers |= MOD_ALT,
            "win" | "super" => modifiers |= MOD_WIN,
            key => {
                if vk.is_some() {
                    return None;
                }
                vk = Some(parse_key(key)?);
            }
        }
    }
    if modifiers == 0 {
        return None;
    }
    Some(Hotkey { modifiers: modifiers | MOD_NOREPEAT, vk: vk? })
}

fn parse_key(key: &str) -> Option<u32> {
    let bytes = key.as_bytes();
    if let [c] = bytes {
        return c.is_ascii_alphanumeric().then(|| c.to_ascii_uppercase() as u32);
    }
    let n: u32 = key.strip_prefix('f')?.parse().ok()?;
    (1..=24).contains(&n).then(|| VK_F1 + n - 1)
}

#[cfg(windows)]
pub fn register(
    hwnd: windows::Win32::Foundation::HWND,
    id: i32,
    hotkey: Hotkey,
) -> windows::core::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, HOT_KEY_MODIFIERS};
    unsafe { RegisterHotKey(hwnd, id, HOT_KEY_MODIFIERS(hotkey.modifiers), hotkey.vk) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifiers_and_letter() {
        assert_eq!(
            parse("Ctrl+Shift+B"),
            Some(Hotkey { modifiers: MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, vk: b'B' as u32 })
        );
    }

    #[test]
    fn parses_function_keys_and_digits_case_insensitively() {
        assert_eq!(parse("alt + f9").unwrap().vk, VK_F1 + 8);
        assert_eq!(parse("WIN+7").unwrap(), Hotkey { modifiers: MOD_WIN | MOD_NOREPEAT, vk: b'7' as u32 });
    }

    #[test]
    fn rejects_invalid_hotkeys() {
        assert_eq!(parse("B"), None); // no modifier
        assert_eq!(parse("Ctrl"), None); // no key
        assert_eq!(parse("Ctrl+A+B"), None); // two keys
        assert_eq!(parse("Ctrl+F25"), None);
        assert_eq!(parse("Ctrl+Esc"), None);
        assert_eq!(parse(""), None);
    }
}
