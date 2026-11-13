use crate::annotations::{Annotation, composite_annotations};
use crate::config::AppConfig;
use anyhow::{Context, Result};
use arboard::{Clipboard, ImageData};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::{DynamicImage, Frame, ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Physical rectangle in screen or buffer coordinates.
///
/// Can represent either:
/// - Signed virtual desktop coordinates (where `x` and `y` can be negative).
/// - Contiguous buffer coordinates (where `x >= 0` and `y >= 0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl ScreenRect {
    /// Creates a physical `ScreenRect` in buffer coordinates from two logical GPUI points
    /// and a monitor scale factor, clamped to `max_width` and `max_height`.
    pub fn from_logical_points(
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        scale_factor: f32,
        max_width: u32,
        max_height: u32,
    ) -> Self {
        let s = if scale_factor <= 0.0 { 1.0 } else { scale_factor };
        let mw = max_width as f32;
        let mh = max_height as f32;

        let p1_x = (x1 * s).clamp(0.0, mw);
        let p1_y = (y1 * s).clamp(0.0, mh);
        let p2_x = (x2 * s).clamp(0.0, mw);
        let p2_y = (y2 * s).clamp(0.0, mh);

        let min_x = p1_x.min(p2_x).floor() as i32;
        let min_y = p1_y.min(p2_y).floor() as i32;
        let max_x = p1_x.max(p2_x).ceil() as i32;
        let max_y = p1_y.max(p2_y).ceil() as i32;

        let clamped_min_x = min_x.clamp(0, max_width.saturating_sub(1) as i32);
        let clamped_min_y = min_y.clamp(0, max_height.saturating_sub(1) as i32);
        let clamped_max_x = max_x.clamp(clamped_min_x + 1, max_width as i32);
        let clamped_max_y = max_y.clamp(clamped_min_y + 1, max_height as i32);

        Self {
            x: clamped_min_x,
            y: clamped_min_y,
            width: (clamped_max_x - clamped_min_x) as u32,
            height: (clamped_max_y - clamped_min_y) as u32,
        }
    }

    /// Converts physical pixel coordinates to logical GPUI DIPs `(x, y, width, height)`.
    pub fn to_logical(&self, scale_factor: f32) -> (f32, f32, f32, f32) {
        let s = if scale_factor <= 0.0 { 1.0 } else { scale_factor };
        (
            self.x as f32 / s,
            self.y as f32 / s,
            self.width as f32 / s,
            self.height as f32 / s,
        )
    }

    /// Returns true if the selection rectangle is large enough to be an intentional drag.
    pub fn is_meaningful(&self) -> bool {
        self.width >= 8 && self.height >= 8
    }

    /// Translates this rectangle from signed virtual desktop coordinates to non-negative buffer coordinates.
    pub fn to_buffer_coords(&self, vx: i32, vy: i32) -> Self {
        Self {
            x: self.x - vx,
            y: self.y - vy,
            width: self.width,
            height: self.height,
        }
    }

    /// Translates this rectangle from non-negative buffer coordinates to signed virtual desktop coordinates.
    pub fn to_virtual_coords(&self, vx: i32, vy: i32) -> Self {
        Self {
            x: self.x + vx,
            y: self.y + vy,
            width: self.width,
            height: self.height,
        }
    }

    /// Returns true if the given point is contained within this rectangle.
    pub fn contains_point(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.width as i32
            && py >= self.y
            && py < self.y + self.height as i32
    }

    /// Returns true if this rectangle intersects another rectangle.
    pub fn intersects(&self, other: &Self) -> bool {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.width as i32).min(other.x + other.width as i32);
        let y2 = (self.y + self.height as i32).min(other.y + other.height as i32);
        x2 > x1 && y2 > y1
    }

    /// Calculates the intersection of two rectangles, or None if disjoint.
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.width as i32).min(other.x + other.width as i32);
        let y2 = (self.y + self.height as i32).min(other.y + other.height as i32);
        if x2 > x1 && y2 > y1 {
            Some(Self {
                x: x1,
                y: y1,
                width: (x2 - x1) as u32,
                height: (y2 - y1) as u32,
            })
        } else {
            None
        }
    }

    /// Calculates the intersection area in square pixels.
    pub fn intersection_area(&self, other: &Self) -> u64 {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.width as i32).min(other.x + other.width as i32);
        let y2 = (self.y + self.height as i32).min(other.y + other.height as i32);
        if x2 > x1 && y2 > y1 {
            (x2 - x1) as u64 * (y2 - y1) as u64
        } else {
            0
        }
    }

    /// Clamps this rectangle so it is fully contained inside `bounds`.
    pub fn clamp_to(&self, bounds: &Self) -> Self {
        let right_bound = bounds.x + bounds.width as i32;
        let bottom_bound = bounds.y + bounds.height as i32;
        let clamped_x = self.x.clamp(bounds.x, (right_bound - self.width as i32).max(bounds.x));
        let clamped_y = self.y.clamp(bounds.y, (bottom_bound - self.height as i32).max(bounds.y));
        Self {
            x: clamped_x,
            y: clamped_y,
            width: self.width.min(bounds.width),
            height: self.height.min(bounds.height),
        }
    }
}

/// Metadata describing an attached physical monitor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorInfo {
    pub display_id: u64,        // Platform HMONITOR / GPUI DisplayId
    pub rect: ScreenRect,       // In virtual desktop coordinates
    pub work_rect: ScreenRect,  // In virtual desktop coordinates
    pub is_primary: bool,
    pub scale_factor: f32,
}

impl MonitorInfo {
    pub fn buffer_rect(&self, vx: i32, vy: i32) -> ScreenRect {
        self.rect.to_buffer_coords(vx, vy)
    }

    pub fn buffer_work_rect(&self, vx: i32, vy: i32) -> ScreenRect {
        self.work_rect.to_buffer_coords(vx, vy)
    }

    pub fn contains_virtual_point(&self, px: i32, py: i32) -> bool {
        self.rect.contains_point(px, py)
    }

    pub fn contains_buffer_point(&self, bx: i32, by: i32, vx: i32, vy: i32) -> bool {
        self.rect.contains_point(bx + vx, by + vy)
    }
}

/// Represents a captured virtual desktop frame held in memory while the WinLook overlay is active.
/// Stores the raw BGRA buffer directly so opening the overlay takes <5ms with zero full-screen RGBA conversion.
#[derive(Clone)]
pub struct CapturedFrame {
    pub bgra_raw: Arc<Vec<u8>>,
    pub render_image: Arc<gpui::RenderImage>,
    pub image: Arc<gpui::RenderImage>, // Contract alias
    pub width: u32,
    pub height: u32,
    pub vx: i32,
    pub vy: i32,
    pub scale_factor: f32,
    pub monitors: Vec<MonitorInfo>,
}

impl CapturedFrame {
    /// Logical width in GPUI DIPs.
    pub fn logical_width(&self) -> f32 {
        self.width as f32 / self.scale_factor.max(1.0)
    }

    /// Logical height in GPUI DIPs.
    pub fn logical_height(&self) -> f32 {
        self.height as f32 / self.scale_factor.max(1.0)
    }

    /// Full buffer `ScreenRect` in physical pixels.
    pub fn full_rect(&self) -> ScreenRect {
        ScreenRect {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        }
    }

    /// Virtual screen `ScreenRect` in virtual desktop coordinates.
    pub fn virtual_rect(&self) -> ScreenRect {
        ScreenRect {
            x: self.vx,
            y: self.vy,
            width: self.width,
            height: self.height,
        }
    }

    /// Finds the monitor that contains the specified buffer coordinates.
    pub fn monitor_for_buffer_point(&self, bx: i32, by: i32) -> Option<&MonitorInfo> {
        let virt_x = bx + self.vx;
        let virt_y = by + self.vy;
        self.monitors.iter().find(|m| m.rect.contains_point(virt_x, virt_y))
    }

    /// Returns the primary monitor metadata, if enumerated.
    pub fn primary_monitor(&self) -> Option<&MonitorInfo> {
        self.monitors.iter().find(|m| m.is_primary)
    }

    /// Slices a single monitor's exact pixel region out of this virtual-desktop capture,
    /// producing a 1:1 `CapturedFrame` with that monitor's own dimensions, origin, and DPI scale.
    pub fn frame_for_monitor(&self, monitor: &MonitorInfo) -> Result<Self> {
        let buf_rect = monitor.buffer_rect(self.vx, self.vy);
        let x0 = (buf_rect.x.max(0) as u32).min(self.width.saturating_sub(1));
        let y0 = (buf_rect.y.max(0) as u32).min(self.height.saturating_sub(1));
        let w = buf_rect.width.min(self.width.saturating_sub(x0)).max(1);
        let h = buf_rect.height.min(self.height.saturating_sub(y0)).max(1);

        let src_stride = (self.width as usize) * 4;
        let row_bytes = (w as usize) * 4;
        let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];

        for row in 0..(h as usize) {
            let src_start = ((y0 as usize) + row) * src_stride + (x0 as usize) * 4;
            let dst_start = row * row_bytes;
            bgra[dst_start..dst_start + row_bytes]
                .copy_from_slice(&self.bgra_raw[src_start..src_start + row_bytes]);
        }

        let bgra_arc = Arc::new(bgra.clone());
        let bgra_img = RgbaImage::from_raw(w, h, bgra)
            .context("Failed to construct per-monitor GPUI BGRA frame")?;
        let render_image = Arc::new(gpui::RenderImage::new(vec![Frame::new(bgra_img)]));

        Ok(Self {
            bgra_raw: bgra_arc,
            render_image: render_image.clone(),
            image: render_image,
            width: w,
            height: h,
            vx: monitor.rect.x,
            vy: monitor.rect.y,
            scale_factor: monitor.scale_factor.max(1.0),
            monitors: vec![*monitor],
        })
    }

    /// Crops a sub-region of the captured screen and converts only the cropped pixels from BGRA to RGBA.
    pub fn crop(&self, rect: ScreenRect) -> DynamicImage {
        let max_w = self.width;
        let max_h = self.height;
        let x0 = (rect.x.max(0) as u32).min(max_w.saturating_sub(1));
        let y0 = (rect.y.max(0) as u32).min(max_h.saturating_sub(1));
        let w = rect.width.min(max_w.saturating_sub(x0)).max(1);
        let h = rect.height.min(max_h.saturating_sub(y0)).max(1);

        let stride = (max_w as usize) * 4;
        let row_bytes = (w as usize) * 4;
        let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];

        for row in 0..(h as usize) {
            let src_start = ((y0 as usize) + row) * stride + (x0 as usize) * 4;
            let dst_start = row * row_bytes;
            let src_slice = &self.bgra_raw[src_start..src_start + row_bytes];
            let dst_slice = &mut rgba[dst_start..dst_start + row_bytes];

            for (src_px, dst_px) in src_slice.chunks_exact(4).zip(dst_slice.chunks_exact_mut(4)) {
                dst_px[0] = src_px[2]; // R
                dst_px[1] = src_px[1]; // G
                dst_px[2] = src_px[0]; // B
                dst_px[3] = 255;       // A
            }
        }

        let img = RgbaImage::from_raw(w, h, rgba).expect("valid cropped dimensions");
        DynamicImage::ImageRgba8(img)
    }

    /// Crops a sub-region and composites any drawn `annotations` onto the cropped image.
    pub fn crop_with_annotations(&self, rect: ScreenRect, annotations: &[Annotation]) -> DynamicImage {
        let mut img = self.crop(rect);
        if !annotations.is_empty() {
            if let Some(rgba) = img.as_mut_rgba8() {
                composite_annotations(rgba, (rect.x, rect.y), self.scale_factor, annotations);
            }
        }
        img
    }

    /// Encodes a cropped region as PNG bytes for `Windows.Media.Ocr`, returning `(png_bytes, upscale_factor)`.
    /// Automatically upscales small selections 2x so `Windows.Media.Ocr` accurately recognizes dense Japanese/CJK characters.
    pub fn encode_crop_png_for_ocr(&self, rect: ScreenRect) -> Result<(Vec<u8>, f32)> {
        let cropped = self.crop(rect);
        let (final_img, scale) = if cropped.height() < 120 || cropped.width() < 240 {
            let up = cropped.resize(
                cropped.width() * 2,
                cropped.height() * 2,
                image::imageops::FilterType::Triangle,
            );
            (up, 2.0_f32)
        } else {
            (cropped, 1.0_f32)
        };

        let mut buf = Cursor::new(Vec::with_capacity(65536));
        final_img
            .write_to(&mut buf, ImageFormat::Png)
            .context("Failed to encode cropped region as PNG for OCR")?;
        Ok((buf.into_inner(), scale))
    }

    /// Encodes the full captured frame as PNG bytes.
    pub fn encode_png_bytes(&self) -> Result<Vec<u8>> {
        let (bytes, _) = self.encode_crop_png_for_ocr(self.full_rect())?;
        Ok(bytes)
    }

    /// Encodes a cropped region as a Base64 WebP data URL (`data:image/webp;base64,...`)
    /// downscaled to at most `max_edge` (default 1280px) for fast DeepSeek Vision requests.
    pub fn crop_to_vision_data_url(&self, rect: ScreenRect, max_edge: u32) -> Result<String> {
        self.crop_to_vision_data_url_with_annotations(rect, max_edge, &[])
    }

    /// Encodes a cropped region with composited annotations as a Base64 WebP data URL.
    pub fn crop_to_vision_data_url_with_annotations(
        &self,
        rect: ScreenRect,
        max_edge: u32,
        annotations: &[Annotation],
    ) -> Result<String> {
        let cropped = self.crop_with_annotations(rect, annotations);
        let resized = if cropped.width() > max_edge || cropped.height() > max_edge {
            cropped.resize(max_edge, max_edge, image::imageops::FilterType::Lanczos3)
        } else {
            cropped
        };

        let mut buf = Cursor::new(Vec::new());
        resized
            .write_to(&mut buf, ImageFormat::WebP)
            .context("Failed to encode cropped region as WebP")?;

        let b64 = BASE64.encode(buf.into_inner());
        Ok(format!("data:image/webp;base64,{}", b64))
    }

    /// Encodes a cropped region as JPEG bytes for Google Lens upload.
    pub fn crop_to_jpeg_bytes(&self, rect: ScreenRect) -> Result<Vec<u8>> {
        self.crop_to_jpeg_bytes_with_annotations(rect, &[])
    }

    /// Encodes a cropped region with composited annotations as JPEG bytes for Google Lens upload.
    pub fn crop_to_jpeg_bytes_with_annotations(
        &self,
        rect: ScreenRect,
        annotations: &[Annotation],
    ) -> Result<Vec<u8>> {
        let cropped = self.crop_with_annotations(rect, annotations);
        let mut buf = Cursor::new(Vec::new());
        cropped
            .write_to(&mut buf, ImageFormat::Jpeg)
            .context("Failed to encode cropped region as JPEG")?;
        Ok(buf.into_inner())
    }

    /// Copies the cropped region image directly to the Windows clipboard.
    pub fn copy_crop_to_clipboard(&self, rect: ScreenRect) -> Result<()> {
        self.copy_crop_to_clipboard_with_annotations(rect, &[])
    }

    /// Copies the cropped region image (with composited annotations) directly to the Windows clipboard.
    pub fn copy_crop_to_clipboard_with_annotations(
        &self,
        rect: ScreenRect,
        annotations: &[Annotation],
    ) -> Result<()> {
        let cropped = self.crop_with_annotations(rect, annotations).into_rgba8();
        let width = cropped.width() as usize;
        let height = cropped.height() as usize;
        let bytes = cropped.into_raw();

        let mut clipboard = Clipboard::new().context("Failed to open Windows clipboard")?;
        clipboard
            .set_image(ImageData {
                width,
                height,
                bytes: Cow::Owned(bytes),
            })
            .context("Failed to copy cropped image to clipboard")?;
        Ok(())
    }

    /// Saves the cropped region as a PNG screenshot in the configured screenshots directory,
    /// and optionally copies it to the Windows clipboard.
    pub fn save_crop_to_disk(&self, rect: ScreenRect, config: &AppConfig) -> Result<PathBuf> {
        self.save_crop_to_disk_with_annotations(rect, config, &[])
    }

    /// Saves the cropped region (with composited annotations) as a PNG screenshot in the configured
    /// screenshots directory, and optionally copies it to the Windows clipboard.
    pub fn save_crop_to_disk_with_annotations(
        &self,
        rect: ScreenRect,
        config: &AppConfig,
        annotations: &[Annotation],
    ) -> Result<PathBuf> {
        let dir = config.ensure_screenshots_dir()?;
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let filename = format!("WinLook_{}_{}x{}.png", secs, rect.width, rect.height);
        let path = dir.join(filename);

        let cropped = self.crop_with_annotations(rect, annotations);
        cropped
            .save_with_format(&path, ImageFormat::Png)
            .with_context(|| format!("Failed to save screenshot to {}", path.display()))?;

        if config.auto_copy_on_save {
            let _ = self.copy_crop_to_clipboard_with_annotations(rect, annotations);
        }

        Ok(path)
    }
}

/// Copies plain text to the Windows clipboard.
pub fn copy_text_to_clipboard(text: &str) -> Result<()> {
    let mut clipboard = Clipboard::new().context("Failed to open Windows clipboard")?;
    clipboard
        .set_text(text.to_string())
        .context("Failed to copy text to Windows clipboard")?;
    Ok(())
}

/// Initializes Windows Per-Monitor DPI awareness (V2 preferred, falling back to V1).
#[cfg(target_os = "windows")]
pub fn init_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        SetProcessDpiAwarenessContext,
    };
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE);
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn init_dpi_awareness() {}

/// Enumerates all active display monitors, their physical and work rectangles, and per-monitor scale factors.
#[cfg(target_os = "windows")]
pub fn enumerate_monitors() -> Vec<MonitorInfo> {
    use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

    let mut monitors = Vec::new();

    unsafe extern "system" fn enum_proc(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let list = &mut *(lparam.0 as *mut Vec<MonitorInfo>);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };

        if GetMonitorInfoW(hmonitor, &mut mi).as_bool() {
            let is_primary = (mi.dwFlags & MONITORINFOF_PRIMARY) != 0;
            let mut dpix = 96u32;
            let mut dpiy = 96u32;
            let scale_factor = if GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpix, &mut dpiy).is_ok()
                && dpix > 0
            {
                (dpix as f32 / 96.0).max(1.0)
            } else {
                1.0
            };

            let rect = ScreenRect {
                x: mi.rcMonitor.left,
                y: mi.rcMonitor.top,
                width: (mi.rcMonitor.right - mi.rcMonitor.left).max(0) as u32,
                height: (mi.rcMonitor.bottom - mi.rcMonitor.top).max(0) as u32,
            };

            let work_rect = ScreenRect {
                x: mi.rcWork.left,
                y: mi.rcWork.top,
                width: (mi.rcWork.right - mi.rcWork.left).max(0) as u32,
                height: (mi.rcWork.bottom - mi.rcWork.top).max(0) as u32,
            };

            list.push(MonitorInfo {
                display_id: hmonitor.0 as u64,
                rect,
                work_rect,
                is_primary,
                scale_factor,
            });
        }
        BOOL(1)
    }

    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(enum_proc),
            LPARAM(&mut monitors as *mut _ as isize),
        );
    }

    monitors
}

#[cfg(not(target_os = "windows"))]
pub fn enumerate_monitors() -> Vec<MonitorInfo> {
    Vec::new()
}

/// Captures the full virtual screen across all connected monitors on Windows using Win32 GDI BitBlt.
#[cfg(target_os = "windows")]
pub fn capture_virtual_screen() -> Result<CapturedFrame> {
    use windows::Win32::Graphics::Gdi::{
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
    };
    use windows::Win32::UI::HiDpi::GetDpiForSystem;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetDesktopWindow, GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN,
        SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };

    init_dpi_awareness();

    unsafe {
        let mut vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let mut vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let mut vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let mut vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);

        // Fallback for single-monitor or headless environment
        if vw <= 0 || vh <= 0 {
            vx = 0;
            vy = 0;
            vw = GetSystemMetrics(SM_CXSCREEN).max(1);
            vh = GetSystemMetrics(SM_CYSCREEN).max(1);
        }

        let mut monitors = enumerate_monitors();
        if monitors.is_empty() {
            let sys_dpi = GetDpiForSystem();
            let scale = if sys_dpi > 0 { (sys_dpi as f32 / 96.0).max(1.0) } else { 1.0 };
            monitors.push(MonitorInfo {
                display_id: 0,
                rect: ScreenRect { x: vx, y: vy, width: vw as u32, height: vh as u32 },
                work_rect: ScreenRect { x: vx, y: vy, width: vw as u32, height: vh as u32 },
                is_primary: true,
                scale_factor: scale,
            });
        }

        let primary_scale = monitors
            .iter()
            .find(|m| m.is_primary)
            .map(|m| m.scale_factor)
            .unwrap_or_else(|| {
                let dpi = GetDpiForSystem();
                if dpi > 0 { (dpi as f32 / 96.0).max(1.0) } else { 1.0 }
            });

        let hwnd = GetDesktopWindow();
        let mut hdc_screen = GetDC(None);
        if hdc_screen.is_invalid() {
            hdc_screen = GetDC(hwnd);
        }
        let hdc_mem = CreateCompatibleDC(hdc_screen);
        let hbm = CreateCompatibleBitmap(hdc_screen, vw, vh);
        let old_obj = SelectObject(hdc_mem, hbm);

        // BitBlt full virtual desktop extent from (vx, vy) to memory DC (0, 0)
        let _ = BitBlt(hdc_mem, 0, 0, vw, vh, hdc_screen, vx, vy, SRCCOPY);

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: vw,
                biHeight: -vh, // Top-down DIB
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bgra = vec![0u8; (vw * vh * 4) as usize];
        GetDIBits(
            hdc_mem,
            hbm,
            0,
            vh as u32,
            Some(bgra.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        SelectObject(hdc_mem, old_obj);
        let _ = DeleteObject(hbm);
        let _ = DeleteDC(hdc_mem);
        ReleaseDC(hwnd, hdc_screen);

        // Alpha pass: ensure opaque alpha across all pixels including dead zones
        let (_, pixels_u32, _) = bgra.align_to_mut::<u32>();
        for px in pixels_u32 {
            *px |= 0xFF00_0000;
        }

        let bgra_arc = Arc::new(bgra.clone());
        let bgra_img = RgbaImage::from_raw(vw as u32, vh as u32, bgra)
            .context("Failed to construct GPUI BGRA frame from GDI buffer")?;
        let render_image = Arc::new(gpui::RenderImage::new(vec![Frame::new(bgra_img)]));

        Ok(CapturedFrame {
            bgra_raw: bgra_arc,
            render_image: render_image.clone(),
            image: render_image,
            width: vw as u32,
            height: vh as u32,
            vx,
            vy,
            scale_factor: primary_scale,
            monitors,
        })
    }
}

/// Backward-compatible wrapper for callers expecting `capture_primary_screen`.
pub fn capture_primary_screen() -> Result<CapturedFrame> {
    capture_virtual_screen()
}

#[cfg(not(target_os = "windows"))]
pub fn capture_virtual_screen() -> Result<CapturedFrame> {
    anyhow::bail!("Native GDI capture requires Windows (`x86_64-pc-windows-msvc`)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screen_rect_signed_coordinate_translations() {
        // Topology: Primary monitor (2560x1440) at (0, 0), Secondary monitor (1920x1080) at (-1920, 0)
        let vx = -1920;
        let vy = 0;

        // Secondary monitor rect in virtual coordinates
        let sec_virt = ScreenRect { x: -1920, y: 0, width: 1920, height: 1080 };
        let sec_buf = sec_virt.to_buffer_coords(vx, vy);
        assert_eq!(sec_buf, ScreenRect { x: 0, y: 0, width: 1920, height: 1080 });
        assert_eq!(sec_buf.to_virtual_coords(vx, vy), sec_virt);

        // Primary monitor rect in virtual coordinates
        let prim_virt = ScreenRect { x: 0, y: 0, width: 2560, height: 1440 };
        let prim_buf = prim_virt.to_buffer_coords(vx, vy);
        assert_eq!(prim_buf, ScreenRect { x: 1920, y: 0, width: 2560, height: 1440 });
        assert_eq!(prim_buf.to_virtual_coords(vx, vy), prim_virt);
    }

    #[test]
    fn test_screen_rect_secondary_above_primary() {
        // Topology: Secondary monitor (1920x1080) placed above primary at (0, -1080)
        let vx = 0;
        let vy = -1080;

        let sec_virt = ScreenRect { x: 0, y: -1080, width: 1920, height: 1080 };
        let sec_buf = sec_virt.to_buffer_coords(vx, vy);
        assert_eq!(sec_buf, ScreenRect { x: 0, y: 0, width: 1920, height: 1080 });
        assert_eq!(sec_buf.to_virtual_coords(vx, vy), sec_virt);

        let prim_virt = ScreenRect { x: 0, y: 0, width: 1920, height: 1080 };
        let prim_buf = prim_virt.to_buffer_coords(vx, vy);
        assert_eq!(prim_buf, ScreenRect { x: 0, y: 1080, width: 1920, height: 1080 });
    }

    #[test]
    fn test_from_logical_points_drag_vectors() {
        let max_w = 4480;
        let max_h = 1440;
        let scale = 1.25;

        // Normal drag
        let r1 = ScreenRect::from_logical_points(100.0, 100.0, 500.0, 400.0, scale, max_w, max_h);
        assert_eq!(r1.x, 125);
        assert_eq!(r1.y, 125);
        assert_eq!(r1.width, 500); // (500*1.25).ceil() - (100*1.25).floor() = 625 - 125 = 500
        assert_eq!(r1.height, 375); // (400*1.25).ceil() - (100*1.25).floor() = 500 - 125 = 375
        assert!(r1.is_meaningful());

        // Reverse drag (bottom-right to top-left) produces identical bounding box
        let r2 = ScreenRect::from_logical_points(500.0, 400.0, 100.0, 100.0, scale, max_w, max_h);
        assert_eq!(r1, r2);

        // Out-of-bounds drag (past negative edges)
        let r3 = ScreenRect::from_logical_points(-100.0, -50.0, 200.0, 300.0, scale, max_w, max_h);
        assert_eq!(r3.x, 0);
        assert_eq!(r3.y, 0);
        assert!(r3.x + r3.width as i32 <= max_w as i32);
        assert!(r3.y + r3.height as i32 <= max_h as i32);

        // Out-of-bounds drag (past right/bottom edges)
        let r4 = ScreenRect::from_logical_points(100.0, 100.0, 6000.0, 3000.0, scale, max_w, max_h);
        assert_eq!(r4.x + r4.width as i32, max_w as i32);
        assert_eq!(r4.y + r4.height as i32, max_h as i32);

        // Sub-8px drag is not meaningful
        let r_small = ScreenRect::from_logical_points(10.0, 10.0, 12.0, 12.0, 1.0, max_w, max_h);
        assert!(!r_small.is_meaningful());
    }

    #[test]
    fn test_scrim_partition_geometry() {
        let lw = 3840.0;
        let lh = 2160.0;
        let scale = 1.0;

        let sel = ScreenRect { x: 400, y: 300, width: 1200, height: 800 };
        let (sx, sy, sw, sh) = sel.to_logical(scale);

        let sel_left = sx.clamp(0.0, lw);
        let sel_top = sy.clamp(0.0, lh);
        let sel_right = (sx + sw).clamp(sel_left, lw);
        let sel_bottom = (sy + sh).clamp(sel_top, lh);
        let sel_h = sel_bottom - sel_top;
        let sel_w = sel_right - sel_left;

        let area_top = lw * sel_top;
        let area_bottom = lw * (lh - sel_bottom);
        let area_left = sel_left * sel_h;
        let area_right = (lw - sel_right) * sel_h;
        let area_sel = sel_w * sel_h;

        let total_area = area_top + area_bottom + area_left + area_right + area_sel;
        assert!((total_area - (lw * lh)).abs() < 1e-4);
    }

    #[test]
    fn test_captured_frame_crop_bounds_safety() {
        let width = 100u32;
        let height = 100u32;
        let bgra = vec![255u8; (width * height * 4) as usize];
        let bgra_arc = Arc::new(bgra.clone());
        let bgra_img = RgbaImage::from_raw(width, height, bgra).unwrap();
        let render_image = Arc::new(gpui::RenderImage::new(vec![Frame::new(bgra_img)]));

        let frame = CapturedFrame {
            bgra_raw: bgra_arc,
            render_image: render_image.clone(),
            image: render_image,
            width,
            height,
            vx: -50,
            vy: -50,
            scale_factor: 1.0,
            monitors: Vec::new(),
        };

        // Out of bounds crop should clamp safely without panic
        let out_rect = ScreenRect { x: -10, y: -10, width: 200, height: 200 };
        let cropped = frame.crop(out_rect);
        assert_eq!(cropped.width(), 100);
        assert_eq!(cropped.height(), 100);

        // Beyond width/height
        let far_rect = ScreenRect { x: 150, y: 150, width: 50, height: 50 };
        let cropped_far = frame.crop(far_rect);
        assert_eq!(cropped_far.width(), 1);
        assert_eq!(cropped_far.height(), 1);
    }

    #[test]
    fn test_screen_rect_intersection_and_clamp() {
        let r1 = ScreenRect { x: 100, y: 100, width: 200, height: 200 };
        let r2 = ScreenRect { x: 200, y: 200, width: 200, height: 200 };
        assert!(r1.intersects(&r2));
        let inter = r1.intersection(&r2).unwrap();
        assert_eq!(inter, ScreenRect { x: 200, y: 200, width: 100, height: 100 });
        assert_eq!(r1.intersection_area(&r2), 10000);

        let disjoint = ScreenRect { x: 400, y: 400, width: 50, height: 50 };
        assert!(!r1.intersects(&disjoint));
        assert!(r1.intersection(&disjoint).is_none());
        assert_eq!(r1.intersection_area(&disjoint), 0);

        let bounds = ScreenRect { x: 0, y: 0, width: 500, height: 500 };
        let outside = ScreenRect { x: 450, y: 450, width: 100, height: 100 };
        let clamped = outside.clamp_to(&bounds);
        assert_eq!(clamped.x, 400);
        assert_eq!(clamped.y, 400);
        assert_eq!(clamped.width, 100);
        assert_eq!(clamped.height, 100);
    }

    #[test]
    fn test_frame_for_monitor_ultrawide_mixed_dpi_and_negative_coords() {
        // Topology:
        // - Primary ultrawide monitor: 344x144 at (0, 0), scale 1.0 (scaled down 10x for fast test)
        // - Secondary monitor above & left: 192x108 at (-100, -108), scale 1.5
        // Virtual desktop: vx = -100, vy = -108, vw = 444, vh = 252
        let vx = -100;
        let vy = -108;
        let vw = 444u32;
        let vh = 252u32;

        let prim = MonitorInfo {
            display_id: 1,
            rect: ScreenRect { x: 0, y: 0, width: 344, height: 144 },
            work_rect: ScreenRect { x: 0, y: 0, width: 344, height: 140 },
            is_primary: true,
            scale_factor: 1.0,
        };
        let sec = MonitorInfo {
            display_id: 2,
            rect: ScreenRect { x: -100, y: -108, width: 192, height: 108 },
            work_rect: ScreenRect { x: -100, y: -108, width: 192, height: 104 },
            is_primary: false,
            scale_factor: 1.5,
        };

        // Fill buffer with 0 (black dead zones), then paint distinct BGRA values for each monitor
        let mut bgra = vec![0u8; (vw * vh * 4) as usize];
        let stride = (vw as usize) * 4;

        // Paint primary monitor [x_buf = 100..444, y_buf = 108..252] with B=10, G=20, R=30, A=255
        for y in 108..252 {
            for x in 100..444 {
                let idx = y * stride + x * 4;
                bgra[idx] = 10;
                bgra[idx + 1] = 20;
                bgra[idx + 2] = 30;
                bgra[idx + 3] = 255;
            }
        }

        // Paint secondary monitor [x_buf = 0..192, y_buf = 0..108] with B=40, G=50, R=60, A=255
        for y in 0..108 {
            for x in 0..192 {
                let idx = y * stride + x * 4;
                bgra[idx] = 40;
                bgra[idx + 1] = 50;
                bgra[idx + 2] = 60;
                bgra[idx + 3] = 255;
            }
        }

        let bgra_arc = Arc::new(bgra.clone());
        let bgra_img = RgbaImage::from_raw(vw, vh, bgra).unwrap();
        let render_image = Arc::new(gpui::RenderImage::new(vec![Frame::new(bgra_img)]));

        let full = CapturedFrame {
            bgra_raw: bgra_arc,
            render_image: render_image.clone(),
            image: render_image,
            width: vw,
            height: vh,
            vx,
            vy,
            scale_factor: 1.0,
            monitors: vec![prim, sec],
        };

        let prim_frame = full.frame_for_monitor(&prim).unwrap();
        assert_eq!(prim_frame.width, 344);
        assert_eq!(prim_frame.height, 144);
        assert_eq!(prim_frame.vx, 0);
        assert_eq!(prim_frame.vy, 0);
        assert_eq!(prim_frame.scale_factor, 1.0);
        assert_eq!(prim_frame.logical_width(), 344.0);
        assert_eq!(prim_frame.logical_height(), 144.0);
        assert_eq!(&prim_frame.bgra_raw[0..4], &[10, 20, 30, 255]);

        let sec_frame = full.frame_for_monitor(&sec).unwrap();
        assert_eq!(sec_frame.width, 192);
        assert_eq!(sec_frame.height, 108);
        assert_eq!(sec_frame.vx, -100);
        assert_eq!(sec_frame.vy, -108);
        assert_eq!(sec_frame.scale_factor, 1.5);
        assert_eq!(sec_frame.logical_width(), 128.0);
        assert_eq!(sec_frame.logical_height(), 72.0);
        assert_eq!(&sec_frame.bgra_raw[0..4], &[40, 50, 60, 255]);
    }
}
