//! GPU rendering of the HUD: Direct2D/DirectWrite into a premultiplied-alpha
//! swap chain presented through DirectComposition. Drawing only happens when
//! the caller asks, so the overlay costs nothing while idle.

use std::mem::ManuallyDrop;

use windows::core::{w, Error, Interface, Result, PCWSTR};
use windows::Win32::Foundation::{E_FAIL, HMODULE, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DeviceContext, ID2D1Factory1, ID2D1SolidColorBrush,
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
    DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

use super::hud::{self, Rgb};
use crate::config::Config;
use crate::model::{self, Device, DeviceKind};

pub struct Renderer {
    ctx: ID2D1DeviceContext,
    swap_chain: IDXGISwapChain1,
    dwrite: IDWriteFactory,
    fonts: Option<Fonts>,
    size: (u32, u32),
    // Kept alive so the swap chain stays attached to the window.
    _dcomp: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
}

struct Fonts {
    scale: f32,
    name: IDWriteTextFormat,
    pct: IDWriteTextFormat,
    icon: IDWriteTextFormat,
    bolt: IDWriteTextFormat,
}

impl Renderer {
    pub fn new(hwnd: HWND, size: (u32, u32)) -> Result<Self> {
        unsafe {
            let d3d = create_d3d_device()?;
            let dxgi_device: IDXGIDevice = d3d.cast()?;

            let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: size.0,
                Height: size.1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap_chain = factory.CreateSwapChainForComposition(&dxgi_device, &desc, None)?;

            let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let ctx = d2d
                .CreateDevice(&dxgi_device)?
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;

            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
            let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = dcomp.CreateVisual()?;
            visual.SetContent(&swap_chain)?;
            target.SetRoot(&visual)?;
            dcomp.Commit()?;

            let renderer = Self {
                ctx,
                swap_chain,
                dwrite: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
                fonts: None,
                size,
                _dcomp: dcomp,
                _target: target,
                _visual: visual,
            };
            renderer.bind_back_buffer()?;
            Ok(renderer)
        }
    }

    /// Draws the HUD at `size` pixels. An error means the device is likely
    /// lost; the caller should drop this renderer and create a new one.
    pub fn draw_hud(&mut self, size: (u32, u32), scale: f32, devices: &[Device], config: &Config) -> Result<()> {
        unsafe {
            if size != self.size {
                self.resize(size)?;
            }
            if self.fonts.as_ref().map_or(true, |f| f.scale != scale) {
                self.fonts = Some(Fonts::new(&self.dwrite, scale)?);
            }
            let Some(fonts) = &self.fonts else { unreachable!() };

            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&color((0.0, 0.0, 0.0), 0.0)));
            let painted = paint_hud(&self.ctx, fonts, size, scale, devices, config);
            let ended = self.ctx.EndDraw(None, None);
            painted?;
            ended?;
            self.swap_chain.Present(1, DXGI_PRESENT(0)).ok()
        }
    }

    unsafe fn resize(&mut self, size: (u32, u32)) -> Result<()> {
        // The back buffer can't be resized while D2D still references it.
        self.ctx.SetTarget(None);
        self.swap_chain.ResizeBuffers(2, size.0, size.1, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SWAP_CHAIN_FLAG(0))?;
        self.size = size;
        self.bind_back_buffer()
    }

    unsafe fn bind_back_buffer(&self) -> Result<()> {
        let surface: IDXGISurface = self.swap_chain.GetBuffer(0)?;
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: ManuallyDrop::new(None),
        };
        let bitmap = self.ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
        self.ctx.SetTarget(&bitmap);
        Ok(())
    }
}

impl Fonts {
    unsafe fn new(dwrite: &IDWriteFactory, scale: f32) -> Result<Self> {
        let ui = w!("Segoe UI");
        let icons = w!("Segoe MDL2 Assets");
        Ok(Self {
            scale,
            name: text_format(dwrite, ui, DWRITE_FONT_WEIGHT_NORMAL, hud::FONT_SIZE * scale, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            pct: text_format(dwrite, ui, DWRITE_FONT_WEIGHT_SEMI_BOLD, hud::FONT_SIZE * scale, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
            icon: text_format(dwrite, icons, DWRITE_FONT_WEIGHT_NORMAL, hud::ICON_FONT_SIZE * scale, DWRITE_TEXT_ALIGNMENT_CENTER)?,
            bolt: text_format(dwrite, icons, DWRITE_FONT_WEIGHT_NORMAL, hud::BOLT_FONT_SIZE * scale, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        })
    }
}

unsafe fn create_d3d_device() -> Result<ID3D11Device> {
    // Fall back to the software rasterizer when no usable GPU driver exists (e.g. some VMs).
    create_d3d_device_with(D3D_DRIVER_TYPE_HARDWARE).or_else(|_| create_d3d_device_with(D3D_DRIVER_TYPE_WARP))
}

unsafe fn create_d3d_device_with(driver: D3D_DRIVER_TYPE) -> Result<ID3D11Device> {
    let mut device = None;
    D3D11CreateDevice(
        None,
        driver,
        HMODULE::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        None,
        D3D11_SDK_VERSION,
        Some(&mut device),
        None,
        None,
    )?;
    device.ok_or_else(|| Error::from(E_FAIL))
}

unsafe fn text_format(
    dwrite: &IDWriteFactory,
    family: PCWSTR,
    weight: DWRITE_FONT_WEIGHT,
    size: f32,
    align: DWRITE_TEXT_ALIGNMENT,
) -> Result<IDWriteTextFormat> {
    let format = dwrite.CreateTextFormat(
        family,
        None,
        weight,
        DWRITE_FONT_STYLE_NORMAL,
        DWRITE_FONT_STRETCH_NORMAL,
        size,
        w!("en-us"),
    )?;
    format.SetTextAlignment(align)?;
    format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
    format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
    // Long device names end in "…" instead of overflowing into the percentage.
    let trimming = DWRITE_TRIMMING {
        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
        delimiter: 0,
        delimiterCount: 0,
    };
    let ellipsis = dwrite.CreateEllipsisTrimmingSign(&format)?;
    format.SetTrimming(&trimming, &ellipsis)?;
    Ok(format)
}

unsafe fn paint_hud(
    ctx: &ID2D1DeviceContext,
    fonts: &Fonts,
    size: (u32, u32),
    s: f32,
    devices: &[Device],
    config: &Config,
) -> Result<()> {
    let op = config.hud.opacity;
    let (w, h) = (size.0 as f32, size.1 as f32);
    let text = brush(ctx, hud::TEXT, op)?;
    let dim = brush(ctx, hud::TEXT_DIM, op)?;

    let background = brush(ctx, hud::BACKGROUND, hud::BACKGROUND_ALPHA * op)?;
    let radius = hud::CORNER_RADIUS * s;
    ctx.FillRoundedRectangle(
        &D2D1_ROUNDED_RECT { rect: rect(0.0, 0.0, w, h), radiusX: radius, radiusY: radius },
        &background,
    );

    let left = hud::PADDING * s;
    let right = w - hud::PADDING * s;
    if devices.is_empty() {
        let top = hud::PADDING * s;
        draw_text(ctx, "No devices found", &fonts.name, rect(left, top, right, top + hud::ROW_HEIGHT * s), &dim);
        return Ok(());
    }

    for (i, device) in devices.iter().enumerate() {
        let top = (hud::PADDING + hud::ROW_HEIGHT * i as f32) * s;
        let bottom = top + hud::ROW_HEIGHT * s;
        let severity = model::severity(device.level, &config.thresholds);
        let level_brush = brush(ctx, hud::severity_color(severity), op)?;

        draw_text(ctx, kind_glyph(device.kind), &fonts.icon, rect(left, top, left + hud::ICON_W * s, bottom), &dim);

        // Battery gauge, right-aligned: body outline, terminal nub, level fill.
        let bar_right = right - hud::NUB_W * s;
        let bar_left = bar_right - hud::BAR_W * s;
        let bar_top = top + (hud::ROW_HEIGHT - hud::BAR_H) / 2.0 * s;
        let bar_bottom = bar_top + hud::BAR_H * s;
        let bar = rect(bar_left, bar_top, bar_right, bar_bottom);
        ctx.DrawRoundedRectangle(
            &D2D1_ROUNDED_RECT { rect: bar, radiusX: 2.0 * s, radiusY: 2.0 * s },
            &dim,
            1.0 * s,
            None,
        );
        let nub_inset = hud::BAR_H * 0.3 * s;
        ctx.FillRectangle(&rect(bar_right, bar_top + nub_inset, right, bar_bottom - nub_inset), &dim);
        if let Some(level) = device.level {
            let inset = 2.5 * s;
            let fill_w = (hud::BAR_W * s - inset * 2.0) * f32::from(level.min(100)) / 100.0;
            ctx.FillRectangle(
                &rect(bar_left + inset, bar_top + inset, bar_left + inset + fill_w, bar_bottom - inset),
                &level_brush,
            );
        }
        if device.charging {
            draw_text(ctx, "\u{E945}", &fonts.bolt, bar, &text);
        }

        let pct_right = bar_left - hud::GAP * s;
        let pct_left = pct_right - hud::PCT_W * s;
        let pct_brush = match severity {
            model::Severity::Warning | model::Severity::Critical => &level_brush,
            model::Severity::Unknown => &dim,
            model::Severity::Ok => &text,
        };
        draw_text(ctx, &hud::level_text(device.level), &fonts.pct, rect(pct_left, top, pct_right, bottom), pct_brush);

        let name_left = left + (hud::ICON_W + hud::GAP) * s;
        draw_text(ctx, &device.name, &fonts.name, rect(name_left, top, pct_left - hud::GAP * s, bottom), &text);
    }
    Ok(())
}

/// Glyphs from Segoe MDL2 Assets (present on Windows 10 and 11).
fn kind_glyph(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Mouse => "\u{E962}",
        DeviceKind::Keyboard => "\u{E765}",
        DeviceKind::Headset => "\u{E7F6}",
        DeviceKind::Controller => "\u{E7FC}",
        DeviceKind::Other => "\u{E702}",
    }
}

unsafe fn draw_text(ctx: &ID2D1DeviceContext, s: &str, format: &IDWriteTextFormat, r: D2D_RECT_F, brush: &ID2D1SolidColorBrush) {
    let wide: Vec<u16> = s.encode_utf16().collect();
    ctx.DrawText(
        &wide,
        format,
        &r,
        brush,
        D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
        DWRITE_MEASURING_MODE_NATURAL,
    );
}

unsafe fn brush(ctx: &ID2D1DeviceContext, rgb: Rgb, alpha: f32) -> Result<ID2D1SolidColorBrush> {
    ctx.CreateSolidColorBrush(&color(rgb, alpha), None)
}

fn color((r, g, b): Rgb, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F { left, top, right, bottom }
}
