//! Notification-area icon and its context menu.

use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, LoadIconW, PostMessageW,
    SetForegroundWindow, TrackPopupMenu, IDI_APPLICATION, MENU_ITEM_FLAGS, MF_CHECKED,
    MF_SEPARATOR, MF_STRING, MF_UNCHECKED, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP,
    WM_NULL,
};

use crate::config::OverlayMode;

/// Callback message the shell sends for mouse events on the icon.
pub const WM_TRAY: u32 = WM_APP + 1;

pub const CMD_TOGGLE_HUD: u32 = 1;
pub const CMD_MODE_ALERTS_ONLY: u32 = 2;
pub const CMD_MODE_HUD_AND_ALERTS: u32 = 3;
pub const CMD_QUIT: u32 = 4;

const ICON_ID: u32 = 1;

pub struct Tray {
    hwnd: HWND,
}

impl Tray {
    pub fn add(hwnd: HWND) -> Result<Self> {
        let tray = Self { hwnd };
        tray.register()?;
        Ok(tray)
    }

    /// Adds the icon again; needed after Explorer restarts ("TaskbarCreated").
    pub fn register(&self) -> Result<()> {
        unsafe {
            let mut nid = self.data();
            nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            nid.uCallbackMessage = WM_TRAY;
            // Placeholder until the app icon is embedded (packaging phase).
            nid.hIcon = LoadIconW(None, IDI_APPLICATION)?;
            copy_wide(&mut nid.szTip, "Battery Overlay");
            Shell_NotifyIconW(NIM_ADD, &nid).ok()
        }
    }

    fn data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: ICON_ID,
            ..Default::default()
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data());
        }
    }
}

/// Shows the context menu at the cursor and returns the chosen `CMD_*`, or 0
/// if dismissed. Runs a modal loop, so callers must not hold app state borrowed.
pub fn show_menu(hwnd: HWND, hud_visible: bool, mode: OverlayMode, hotkey: &str) -> Result<u32> {
    let toggle_label = HSTRING::from(format!("Show HUD\t{hotkey}"));
    let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
    unsafe {
        let menu = CreatePopupMenu()?;
        let item = |flags: MENU_ITEM_FLAGS, id: u32, text: PCWSTR| AppendMenuW(menu, flags, id as usize, text);
        let built = (|| -> Result<()> {
            item(MF_STRING | check(hud_visible), CMD_TOGGLE_HUD, PCWSTR(toggle_label.as_ptr()))?;
            item(MF_SEPARATOR, 0, PCWSTR::null())?;
            item(MF_STRING | check(mode == OverlayMode::AlertsOnly), CMD_MODE_ALERTS_ONLY, w!("Mode: Alerts only"))?;
            item(
                MF_STRING | check(mode == OverlayMode::HudAndAlerts),
                CMD_MODE_HUD_AND_ALERTS,
                w!("Mode: HUD + alerts"),
            )?;
            item(MF_SEPARATOR, 0, PCWSTR::null())?;
            item(MF_STRING, CMD_QUIT, w!("Quit"))
        })();
        let cmd = match built {
            Ok(()) => {
                let mut pt = POINT::default();
                GetCursorPos(&mut pt)?;
                // Without this the menu doesn't close when clicking elsewhere.
                let _ = SetForegroundWindow(hwnd);
                let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, 0, hwnd, None);
                let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
                Ok(cmd.0 as u32)
            }
            Err(e) => Err(e),
        };
        let _ = DestroyMenu(menu);
        cmd
    }
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let max = dst.len() - 1; // keep the terminating NUL
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(max)) {
        *d = c;
    }
}
