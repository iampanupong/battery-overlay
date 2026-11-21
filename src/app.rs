//! Main UI thread: a hidden window owns the tray icon and hotkey and drives the overlay.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetTimer,
    TranslateMessage, DBT_DEVNODES_CHANGED, MSG, WINDOW_EX_STYLE, WM_APP, WM_CONTEXTMENU,
    WM_DESTROY, WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_HOTKEY, WM_LBUTTONDBLCLK, WM_RBUTTONUP,
    WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
};

use crate::config::{self, Config, OverlayMode};
use crate::model::{self, Device};
use crate::overlay::{Overlay, TopmostGuard};
use crate::{hotkey, providers, settings, tray};

const HOTKEY_ID: i32 = 1;
const POLL_TIMER: usize = 1;
/// One-shot re-poll once a burst of device-change messages has settled.
const DEVICE_CHANGE_TIMER: usize = 2;
const DEVICE_CHANGE_DELAY_MS: u32 = 250;
/// Second re-poll: Windows publishes a newly connected device's battery level
/// a moment after the device itself appears.
const DEVICE_SETTLE_TIMER: usize = 3;
const DEVICE_SETTLE_DELAY_MS: u32 = 3000;
/// Posted by the poll thread; `lparam` owns a `Box<Vec<Device>>`.
const WM_POLLED: u32 = WM_APP + 2;

struct App {
    hwnd: HWND,
    config: Config,
    config_path: PathBuf,
    devices: Vec<Device>,
    /// Shows placeholder devices instead of polling (`--demo`).
    demo: bool,
    hud: Overlay,
    hud_visible: bool,
    tray: tray::Tray,
    /// A poll thread is running; its result arrives as `WM_POLLED`.
    polling: bool,
    /// Another poll was requested while one was running.
    repoll: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    /// Posted by the settings process after it saves the config.
    static CONFIG_CHANGED: Cell<u32> = const { Cell::new(0) };
}

/// Runs `f` on the app state. Returns `None` if the state is missing or
/// already borrowed (a re-entrant message), which callers treat as a no-op.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|app| app.try_borrow_mut().ok()?.as_mut().map(f))
}

pub fn run(demo: bool) -> Result<()> {
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
    CONFIG_CHANGED.set(unsafe { RegisterWindowMessageW(&HSTRING::from(settings::CONFIG_CHANGED_MSG)) });

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
        hwnd,
        hud_visible: config.mode == OverlayMode::HudAndAlerts,
        config,
        config_path,
        devices: if demo { model::fake_devices() } else { Vec::new() },
        demo,
        hud,
        tray: tray::Tray::add(hwnd)?,
        polling: false,
        repoll: false,
    };
    app.refresh_hud();
    app.poll();
    if !demo {
        unsafe { SetTimer(hwnd, POLL_TIMER, app.config.poll_interval_secs as u32 * 1000, None) };
    }
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

    /// Re-reads all providers on a short-lived thread, since vendor HID
    /// queries can wait on sleeping devices. At most one runs at a time.
    fn poll(&mut self) {
        if self.demo {
            return;
        }
        if self.polling {
            self.repoll = true;
            return;
        }
        self.polling = true;
        // HWND isn't Send; the raw value is valid to post to from any thread.
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let devices = Box::into_raw(Box::new(providers::poll_all()));
            let posted =
                unsafe { PostMessageW(HWND(hwnd as _), WM_POLLED, WPARAM(0), LPARAM(devices as isize)) };
            if posted.is_err() {
                // The window is gone; nobody will take ownership.
                drop(unsafe { Box::from_raw(devices) });
            }
        });
    }

    /// Takes a poll result and redraws only if something changed.
    fn on_polled(&mut self, devices: Vec<Device>) {
        self.polling = false;
        if devices != self.devices {
            self.devices = devices;
            self.refresh_hud();
        }
        if std::mem::take(&mut self.repoll) {
            self.poll();
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

    /// Applies what the settings window saved. Only the HUD and mode are
    /// live; a changed hotkey or poll interval takes effect on restart.
    fn reload_config(&mut self) {
        let new = config::load(&self.config_path);
        self.hud_visible = config::hud_visible_after_reload(self.config.mode, new.mode, self.hud_visible);
        self.config.mode = new.mode;
        self.config.hud = new.hud;
        self.refresh_hud();
    }
}

/// Starts the settings window as its own process, so egui never loads here.
/// A second one just brings the open window to the front.
fn open_settings() {
    let spawned = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg("--settings").spawn());
    if let Err(e) = spawned {
        eprintln!("could not open settings: {e}");
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
        // The settings process finds this window by its class name.
        let class = HSTRING::from(settings::MAIN_WINDOW_CLASS);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(main_wndproc),
            hInstance: instance,
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);
        // A real (never shown) top-level window rather than a message-only one,
        // so it receives broadcasts like TaskbarCreated and WM_DISPLAYCHANGE.
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            &class,
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
        tray::CMD_SETTINGS => open_settings(),
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
        WM_POLLED => {
            let devices = *Box::from_raw(lparam.0 as *mut Vec<Device>);
            with_app(|a| a.on_polled(devices));
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == POLL_TIMER => {
            with_app(App::poll);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == DEVICE_CHANGE_TIMER => {
            let _ = KillTimer(hwnd, DEVICE_CHANGE_TIMER);
            with_app(App::poll);
            SetTimer(hwnd, DEVICE_SETTLE_TIMER, DEVICE_SETTLE_DELAY_MS, None);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == DEVICE_SETTLE_TIMER => {
            let _ = KillTimer(hwnd, DEVICE_SETTLE_TIMER);
            with_app(App::poll);
            LRESULT(0)
        }
        // A device connected or disconnected; re-arming the timer debounces bursts.
        WM_DEVICECHANGE if wparam.0 == DBT_DEVNODES_CHANGED as usize => {
            SetTimer(hwnd, DEVICE_CHANGE_TIMER, DEVICE_CHANGE_DELAY_MS, None);
            LRESULT(1)
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
        m if m != 0 && m == CONFIG_CHANGED.get() => {
            with_app(App::reload_config);
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
