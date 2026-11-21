//! The overlay window: transparent, click-through, topmost and never focused,
//! so it can sit above a borderless or flip-model fullscreen game without
//! touching the game process.

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::core::{w, Result};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, IsWindowVisible, RegisterClassW,
    SetLayeredWindowAttributes, SetWindowPos, ShowWindow, EVENT_SYSTEM_FOREGROUND, HTTRANSPARENT,
    HWND_TOPMOST, LWA_ALPHA, MA_NOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SW_HIDE, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_MOUSEACTIVATE, WM_NCHITTEST,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::hud::{self, Rect};
use super::render::Renderer;
use crate::config::Config;
use crate::model::Device;

pub struct Overlay {
    hwnd: HWND,
    renderer: Option<Renderer>,
}

impl Overlay {
    pub fn new(instance: HINSTANCE) -> Result<Self> {
        unsafe {
            let class = w!("BatteryOverlay.Overlay");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(overlay_wndproc),
                hInstance: instance,
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                // LAYERED + TRANSPARENT makes mouse input fall through to the game;
                // NOREDIRECTIONBITMAP because all content comes from DirectComposition.
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_NOREDIRECTIONBITMAP,
                class,
                w!("Battery Overlay HUD"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                instance,
                None,
            )?;
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
            Ok(Self { hwnd, renderer: None })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Lays out, positions and redraws the HUD, then shows it without activating.
    pub fn show_hud(&mut self, devices: &[Device], config: &Config) -> Result<()> {
        let (monitor, dpi) = primary_monitor();
        let scale = dpi as f32 / 96.0 * config.hud.scale;
        let size = hud::hud_size(devices.len(), scale);
        let margin = (config.hud.margin as f32 * scale).round() as i32;
        let (x, y) = hud::position(config.hud.corner, monitor, size, margin);
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                x,
                y,
                size.0 as i32,
                size.1 as i32,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )?;
        }

        if self.renderer.is_none() {
            self.renderer = Some(Renderer::new(self.hwnd, size)?);
        }
        let renderer = self.renderer.as_mut().expect("renderer was just created");
        if renderer.draw_hud(size, scale, devices, config).is_err() {
            // Most likely a lost GPU device (driver update, GPU switch): rebuild once.
            let renderer = self.renderer.insert(Renderer::new(self.hwnd, size)?);
            renderer.draw_hud(size, scale, devices, config)?;
        }
        Ok(())
    }

    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.renderer = None;
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Returns the primary monitor's full bounds (not the work area: in fullscreen
/// games the taskbar is hidden, so the HUD should hug the real screen corner)
/// and its effective DPI.
fn primary_monitor() -> (Rect, u32) {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let r = if GetMonitorInfoW(monitor, &mut info).as_bool() {
            info.rcMonitor
        } else {
            Default::default()
        };
        let (mut dpi_x, mut dpi_y) = (96, 96);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_err() {
            dpi_x = 96;
        }
        (Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }, dpi_x)
    }
}

unsafe extern "system" fn overlay_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

static GUARDED_HWND: AtomicIsize = AtomicIsize::new(0);

/// Re-asserts HWND_TOPMOST whenever another app becomes foreground, because
/// games that go fullscreen often push themselves above existing topmost windows.
pub struct TopmostGuard(HWINEVENTHOOK);

impl TopmostGuard {
    pub fn install(hwnd: HWND) -> Self {
        GUARDED_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
        let hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_foreground_changed),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        Self(hook)
    }
}

impl Drop for TopmostGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWinEvent(self.0);
        }
        GUARDED_HWND.store(0, Ordering::Relaxed);
    }
}

unsafe extern "system" fn on_foreground_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    let hwnd = HWND(GUARDED_HWND.load(Ordering::Relaxed) as *mut c_void);
    if !hwnd.is_invalid() && IsWindowVisible(hwnd).as_bool() {
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}
