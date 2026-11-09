use crate::config::AppConfig;
use anyhow::{Context, Result};
use arboard::{Clipboard, ImageData};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::{DynamicImage, Frame, ImageFormat, RgbaImage};
use std::borrow::Cow;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Physical rectangle in screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ScreenRect {
    /// Creates a physical `ScreenRect` from two logical GPUI points and a monitor scale factor.
    pub fn from_logical_points(
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        scale_factor: f32,
        max_width: u32,
        max_height: u32,
    ) -> Self {
        let min_x = (x1.min(x2) * scale_factor).max(0.0) as u32;
        let min_y = (y1.min(y2) * scale_factor).max(0.0) as u32;
        let max_x = ((x1.max(x2) * scale_factor).ceil() as u32).min(max_width);
        let max_y = ((y1.max(y2) * scale_factor).ceil() as u32).min(max_height);

        Self {
            x: min_x.min(max_width.saturating_sub(1)),
            y: min_y.min(max_height.saturating_sub(1)),
            width: max_x.saturating_sub(min_x).max(1),
            height: max_y.saturating_sub(min_y).max(1),
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
}

/// Represents a captured monitor frame held in memory while the WinLook overlay is active.
/// Stores the raw BGRA buffer directly so opening the overlay takes <5ms with zero full-screen RGBA conversion.
#[derive(Clone)]
pub struct CapturedFrame {
    pub bgra_raw: Arc<Vec<u8>>,
    pub render_image: Arc<gpui::RenderImage>,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
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

    /// Full screen `ScreenRect` in physical pixels.
    pub fn full_rect(&self) -> ScreenRect {
        ScreenRect {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        }
    }

    /// Crops a sub-region of the captured screen and converts only the cropped pixels from BGRA to RGBA.
    pub fn crop(&self, rect: ScreenRect) -> DynamicImage {
        let max_w = self.width;
        let max_h = self.height;
        let x0 = rect.x.min(max_w.saturating_sub(1));
        let y0 = rect.y.min(max_h.saturating_sub(1));
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
        let cropped = self.crop(rect);
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
        let cropped = self.crop(rect);
        let mut buf = Cursor::new(Vec::new());
        cropped
            .write_to(&mut buf, ImageFormat::Jpeg)
            .context("Failed to encode cropped region as JPEG")?;
        Ok(buf.into_inner())
    }

    /// Copies the cropped region image directly to the Windows clipboard.
    pub fn copy_crop_to_clipboard(&self, rect: ScreenRect) -> Result<()> {
        let cropped = self.crop(rect).into_rgba8();
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
        let dir = config.ensure_screenshots_dir()?;
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let filename = format!("WinLook_{}_{}x{}.png", secs, rect.width, rect.height);
        let path = dir.join(filename);

        let cropped = self.crop(rect);
        cropped
            .save_with_format(&path, ImageFormat::Png)
            .with_context(|| format!("Failed to save screenshot to {}", path.display()))?;

        if config.auto_copy_on_save {
            let _ = self.copy_crop_to_clipboard(rect);
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

/// Captures the primary screen on Windows using Win32 GDI BitBlt (<5ms) without duplicating RGBA buffers.
#[cfg(target_os = "windows")]
pub fn capture_primary_screen() -> Result<CapturedFrame> {
    use windows::Win32::Graphics::Gdi::{
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
    };
    use windows::Win32::UI::HiDpi::GetDpiForSystem;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetDesktopWindow, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
    };

    unsafe {
        let dpi = GetDpiForSystem();
        let scale_factor = if dpi > 0 {
            (dpi as f32 / 96.0).max(1.0)
        } else {
            1.0
        };

        let hwnd = GetDesktopWindow();
        let mut hdc_screen = GetDC(None);
        if hdc_screen.is_invalid() {
            hdc_screen = GetDC(hwnd);
        }
        let hdc_mem = CreateCompatibleDC(hdc_screen);

        let width = GetSystemMetrics(SM_CXSCREEN).max(1);
        let height = GetSystemMetrics(SM_CYSCREEN).max(1);

        let hbm = CreateCompatibleBitmap(hdc_screen, width, height);
        let old_obj = SelectObject(hdc_mem, hbm);

        let _ = BitBlt(hdc_mem, 0, 0, width, height, hdc_screen, 0, 0, SRCCOPY);

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // Top-down DIB
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bgra = vec![0u8; (width * height * 4) as usize];
        GetDIBits(
            hdc_mem,
            hbm,
            0,
            height as u32,
            Some(bgra.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        SelectObject(hdc_mem, old_obj);
        let _ = DeleteObject(hbm);
        let _ = DeleteDC(hdc_mem);
        ReleaseDC(hwnd, hdc_screen);

        // Fast 32-bit word pass to force alpha = 0xFF across the entire BGRA buffer
        let (_, pixels_u32, _) = bgra.align_to_mut::<u32>();
        for px in pixels_u32 {
            *px |= 0xFF00_0000;
        }

        let bgra_arc = Arc::new(bgra.clone());
        let bgra_img = RgbaImage::from_raw(width as u32, height as u32, bgra)
            .context("Failed to construct GPUI BGRA frame from GDI buffer")?;
        let render_image = Arc::new(gpui::RenderImage::new(vec![Frame::new(bgra_img)]));

        Ok(CapturedFrame {
            bgra_raw: bgra_arc,
            render_image,
            width: width as u32,
            height: height as u32,
            scale_factor,
        })
    }
}

#[cfg(not(target_os = "windows"))]
pub fn capture_primary_screen() -> Result<CapturedFrame> {
    anyhow::bail!("Native GDI capture requires Windows (`x86_64-pc-windows-msvc`)")
}
