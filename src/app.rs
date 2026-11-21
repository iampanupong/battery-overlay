//! Main UI thread: a hidden window owns the tray icon and hotkey and drives the overlay.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use windows::core::{w, Result};
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, TranslateMessage, MSG,
    WINDOW_EX_STYLE, WM_CONTEXTMENU, WM_DESTROY, WM_DISPLAYCHANGE, WM_HOTKEY, WM_LBUTTONDBLCLK,
    WM_RBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
};

use crate::config::{self, Config, OverlayMode};
use crate::model::{self, Device};
use crate::overlay::{Overlay, TopmostGuard};
use crate::{hotkey, tray};

const HOTKEY_ID: i32 = 1;

struct App {
    config: Config,
    config_path: PathBuf,
    devices: Vec<Device>,
    hud: Overlay,
    hud_visible: bool,
    tray: tray::Tray,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

/// Runs `f` on the app state. Returns `None` if the state is missing or
/// already borrowed (a re-entrant message), which callers treat as a no-op.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|app| app.try_borrow_mut().ok()?.as_mut().map(f))
}

pub fn run() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // Held until the process exits; the OS releases it.
    let Some(_instance_lock) = acquire_single_instance()? else {
        return Ok(());
    };

    let config_path = config::config_path();
    let config = config::load(&config_path);
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
    let hwnd = create_main_window(instance)?;
    TASKBAR_CREATED.set(unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) });

    match hotkey::parse(&config.hotkey) {
        Some(hk) => {
            if let Err(e) = hotkey::register(hwnd, HOTKEY_ID, hk) {
                eprintln!("hotkey {} is unavailable (in use by another app?): {e}", config.hotkey);
            }
        }
        None => eprintln!("invalid hotkey in config: {:?}", config.hotkey),
    }

    let hud = Overlay::new(instance)?;
    let _topmost = TopmostGuard::install(hud.hwnd());
    let mut app = App {
        hud_visible: config.mode == OverlayMode::HudAndAlerts,
        config,
        config_path,
        devices: model::fake_devices(),
        hud,
        tray: tray::Tray::add(hwnd)?,
    };
    app.refresh_hud();
    APP.with(|a| *a.borrow_mut() = Some(app));

    let mut msg = MSG::default();
    // GetMessageW returns -1 on error and 0 on WM_QUIT.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // Drops the tray icon and overlay window before the process exits.
    APP.with(|a| a.borrow_mut().take());
    Ok(())
}

impl App {
    fn refresh_hud(&mut self) {
        if !self.hud_visible {
            self.hud.hide();
        } else if let Err(e) = self.hud.show_hud(&self.devices, &self.config) {
            eprintln!("failed to draw HUD: {e}");
        }
    }

    fn toggle_hud(&mut self) {
        self.hud_visible = !self.hud_visible;
        self.refresh_hud();
    }

    fn set_mode(&mut self, mode: OverlayMode) {
        self.config.mode = mode;
        self.hud_visible = mode == OverlayMode::HudAndAlerts;
        if let Err(e) = config::save(&self.config_path, &self.config) {
            eprintln!("could not save config: {e}");
        }
        self.refresh_hud();
    }
}

fn acquire_single_instance() -> Result<Option<HANDLE>> {
    unsafe {
        let mutex = CreateMutexW(None, true, w!("Local\\BatteryOverlay.SingleInstance"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(None);
        }
        Ok(Some(mutex))
    }
}

fn create_main_window(instance: HINSTANCE) -> Result<HWND> {
    unsafe {
        let class = w!("BatteryOverlay.Main");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(main_wndproc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        // A real (never shown) top-level window rather than a message-only one,
        // so it receives broadcasts like TaskbarCreated and WM_DISPLAYCHANGE.
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Battery Overlay"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            instance,
            None,
        )
    }
}

fn on_tray_menu(hwnd: HWND) {
    let Some((hud_visible, mode, hotkey)) =
        with_app(|a| (a.hud_visible, a.config.mode, a.config.hotkey.clone()))
    else {
        return;
    };
    let cmd = match tray::show_menu(hwnd, hud_visible, mode, &hotkey) {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("tray menu failed: {e}");
            return;
        }
    };
    match cmd {
        tray::CMD_TOGGLE_HUD => {
            with_app(App::toggle_hud);
        }
        tray::CMD_MODE_ALERTS_ONLY => {
            with_app(|a| a.set_mode(OverlayMode::AlertsOnly));
        }
        tray::CMD_MODE_HUD_AND_ALERTS => {
            with_app(|a| a.set_mode(OverlayMode::HudAndAlerts));
        }
        tray::CMD_QUIT => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        _ => {}
    }
}

unsafe extern "system" fn main_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        tray::WM_TRAY => {
            match lparam.0 as u32 {
                WM_RBUTTONUP | WM_CONTEXTMENU => on_tray_menu(hwnd),
                WM_LBUTTONDBLCLK => {
                    with_app(App::toggle_hud);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_HOTKEY if wparam.0 == HOTKEY_ID as usize => {
            with_app(App::toggle_hud);
            LRESULT(0)
        }
        // Resolution, DPI or monitor layout changed: re-place the HUD.
        WM_DISPLAYCHANGE => {
            with_app(App::refresh_hud);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.get() => {
            with_app(|a| {
                if let Err(e) = a.tray.register() {
                    eprintln!("could not re-add tray icon: {e}");
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
