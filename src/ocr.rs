use crate::capture::{CapturedFrame, ScreenRect};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Bounding rectangle in physical screen coordinates returned by `Windows.Media.Ocr`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl BoundingBox {
    /// Checks whether this OCR bounding box intersects a physical `ScreenRect`.
    pub fn intersects(&self, rect: &ScreenRect) -> bool {
        let rx1 = rect.x as f32;
        let ry1 = rect.y as f32;
        let rx2 = (rect.x + rect.width) as f32;
        let ry2 = (rect.y + rect.height) as f32;

        let bx1 = self.x;
        let by1 = self.y;
        let bx2 = self.x + self.width;
        let by2 = self.y + self.height;

        let overlap_x = (rx2.min(bx2) - rx1.max(bx1)).max(0.0);
        let overlap_y = (ry2.min(by2) - ry1.max(by1)).max(0.0);

        overlap_x > 2.0 && overlap_y > 2.0
    }

    /// Converts this bounding box into a padded physical `ScreenRect`.
    pub fn to_screen_rect(&self, pad: u32, max_w: u32, max_h: u32) -> ScreenRect {
        let x = (self.x.floor() as u32).saturating_sub(pad);
        let y = (self.y.floor() as u32).saturating_sub(pad);
        let w = ((self.width.ceil() as u32) + pad * 2)
            .min(max_w.saturating_sub(x))
            .max(1);
        let h = ((self.height.ceil() as u32) + pad * 2)
            .min(max_h.saturating_sub(y))
            .max(1);
        ScreenRect {
            x,
            y,
            width: w,
            height: h,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrWordBox {
    pub text: String,
    pub bounds: BoundingBox,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrLineBox {
    pub id: usize,
    pub text: String,
    pub bounds: BoundingBox,
    pub words: Vec<OcrWordBox>,
}

/// Returns true if `c` is a CJK (Japanese Kanji/Hiragana/Katakana, Chinese Hanzi, Korean Hangul, or CJK punctuation) character.
pub fn is_cjk_char(c: char) -> bool {
    matches!(
        c as u32,
        0x3000..=0x303F // CJK Symbols and Punctuation
        | 0x3040..=0x309F // Hiragana
        | 0x30A0..=0x30FF // Katakana
        | 0x31F0..=0x31FF // Katakana Phonetic Extensions
        | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs (Kanji / Hanzi)
        | 0xAC00..=0xD7AF // Hangul Syllables
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0xFF00..=0xFFEF // Halfwidth and Fullwidth Forms
    )
}

/// Counts how many CJK characters appear in `s`.
pub fn count_cjk_chars(s: &str) -> usize {
    s.chars().filter(|&c| is_cjk_char(c)).count()
}

/// Removes artificial spaces that `Windows.Media.Ocr` inserts between adjacent Japanese/Chinese characters
/// while preserving spaces between Latin words.
pub fn clean_cjk_spacing(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == ' ' && i > 0 && i + 1 < chars.len() {
            let prev = chars[i - 1];
            let next = chars[i + 1];
            if is_cjk_char(prev) && is_cjk_char(next) {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Filters all OCR lines that intersect the given `ScreenRect`.
pub fn lines_in_rect(lines: &[OcrLineBox], rect: &ScreenRect) -> Vec<OcrLineBox> {
    lines
        .iter()
        .filter(|line| line.bounds.intersects(rect))
        .cloned()
        .collect()
}

/// Extracts and joins all OCR text lines intersecting the given `ScreenRect`.
pub fn extract_text_in_rect(lines: &[OcrLineBox], rect: &ScreenRect) -> String {
    lines
        .iter()
        .filter(|line| line.bounds.intersects(rect))
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Lists all installed `Windows.Media.Ocr` language tags on this PC (e.g. `["auto", "en-US", "es-MX", "ja"]`).
#[cfg(target_os = "windows")]
pub fn available_ocr_languages() -> Vec<String> {
    use windows::Media::Ocr::OcrEngine;
    let mut tags = vec!["auto".to_string()];
    if let Ok(langs) = OcrEngine::AvailableRecognizerLanguages() {
        for lang in langs {
            if let Ok(tag) = lang.LanguageTag() {
                let s = tag.to_string();
                if !s.is_empty() && !tags.contains(&s) {
                    tags.push(s);
                }
            }
        }
    }
    tags
}

#[cfg(not(target_os = "windows"))]
pub fn available_ocr_languages() -> Vec<String> {
    vec!["auto".to_string()]
}

#[cfg(target_os = "windows")]
fn run_engine_on_bitmap(
    engine: &windows::Media::Ocr::OcrEngine,
    bitmap: &windows::Graphics::Imaging::SoftwareBitmap,
    offset_x: f32,
    offset_y: f32,
    upscale: f32,
) -> Result<Vec<OcrLineBox>> {
    let scale = if upscale <= 0.0 { 1.0 } else { upscale };
    let result = engine.RecognizeAsync(bitmap)?.get()?;
    let lines = result.Lines()?;

    let mut output = Vec::new();
    for (idx, line) in lines.into_iter().enumerate() {
        let raw_line_text = line.Text()?.to_string();
        let line_text = clean_cjk_spacing(&raw_line_text);
        if line_text.trim().is_empty() {
            continue;
        }
        let words_vec = line.Words()?;

        let mut words = Vec::new();
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;

        for word in words_vec {
            let rect = word.BoundingRect()?;
            let wx = offset_x + (rect.X / scale);
            let wy = offset_y + (rect.Y / scale);
            let ww = rect.Width / scale;
            let wh = rect.Height / scale;

            min_x = min_x.min(wx);
            min_y = min_y.min(wy);
            max_x = max_x.max(wx + ww);
            max_y = max_y.max(wy + wh);

            words.push(OcrWordBox {
                text: word.Text()?.to_string(),
                bounds: BoundingBox {
                    x: wx,
                    y: wy,
                    width: ww,
                    height: wh,
                },
            });
        }

        if min_x < max_x && min_y < max_y {
            output.push(OcrLineBox {
                id: idx,
                text: line_text,
                bounds: BoundingBox {
                    x: min_x,
                    y: min_y,
                    width: max_x - min_x,
                    height: max_y - min_y,
                },
                words,
            });
        }
    }

    Ok(output)
}

/// Runs fast, multi-language `Windows.Media.Ocr` on a cropped screen region.
/// When `ocr_lang_setting` is `"auto"`, checks installed CJK engines (`ja`, `zh`, `ko`) alongside the default
/// user profile language so Japanese/Chinese/Korean text is recognized out of the box without switching settings.
#[cfg(target_os = "windows")]
pub fn recognize_cropped_region_sync(
    frame: &CapturedFrame,
    rect: ScreenRect,
    ocr_lang_setting: &str,
) -> Result<Vec<OcrLineBox>> {
    let (png_bytes, upscale) = frame.encode_crop_png_for_ocr(rect)?;
    recognize_png_with_offset_sync(
        &png_bytes,
        rect.x as f32,
        rect.y as f32,
        upscale,
        ocr_lang_setting,
    )
}

#[cfg(target_os = "windows")]
pub fn recognize_png_with_offset_sync(
    png_bytes: &[u8],
    offset_x: f32,
    offset_y: f32,
    upscale: f32,
    ocr_lang_setting: &str,
) -> Result<Vec<OcrLineBox>> {
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::BitmapDecoder;
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};
    use windows::core::HSTRING;

    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(png_bytes)?;
    writer.StoreAsync()?.get()?;
    writer.FlushAsync()?.get()?;
    stream.Seek(0)?;

    let decoder = BitmapDecoder::CreateAsync(&stream)?.get()?;
    let bitmap = decoder.GetSoftwareBitmapAsync()?.get()?;

    let pref = ocr_lang_setting.trim();

    // If the user explicitly selected a specific language tag (e.g. "ja", "en-US", "es-MX")
    if !pref.is_empty() && !pref.eq_ignore_ascii_case("auto") {
        if let Ok(lang) = Language::CreateLanguage(&HSTRING::from(pref)) {
            if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&lang) {
                return run_engine_on_bitmap(&engine, &bitmap, offset_x, offset_y, upscale);
            }
        }
    }

    // "auto" mode:
    // 1. Run primary profile language engine (e.g. en-US / es-MX)
    let default_engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .context("Failed to initialize Windows.Media.Ocr engine")?;
    let default_lines =
        run_engine_on_bitmap(&default_engine, &bitmap, offset_x, offset_y, upscale)
            .unwrap_or_default();

    // 2. Also check installed CJK recognizer languages (`ja`, `zh`, `ko`) so Japanese/Chinese/Korean
    //    text works automatically even when the user's Windows UI language is English or Spanish!
    if let Ok(installed_langs) = OcrEngine::AvailableRecognizerLanguages() {
        let mut best_cjk_lines: Option<( usize, Vec<OcrLineBox> )> = None;

        for lang in installed_langs {
            let tag = lang.LanguageTag().map(|t| t.to_string()).unwrap_or_default();
            let tag_lower = tag.to_ascii_lowercase();
            let is_cjk_engine = tag_lower.starts_with("ja")
                || tag_lower.starts_with("zh")
                || tag_lower.starts_with("ko");

            if is_cjk_engine {
                if let Ok(cjk_engine) = OcrEngine::TryCreateFromLanguage(&lang) {
                    if let Ok(lines) =
                        run_engine_on_bitmap(&cjk_engine, &bitmap, offset_x, offset_y, upscale)
                    {
                        let cjk_count: usize =
                            lines.iter().map(|l| count_cjk_chars(&l.text)).sum();
                        if cjk_count > 0 {
                            if best_cjk_lines
                                .as_ref()
                                .map_or(true, |(best_n, _)| cjk_count > *best_n)
                            {
                                best_cjk_lines = Some((cjk_count, lines));
                            }
                        } else if default_lines.is_empty() && !lines.is_empty() {
                            best_cjk_lines = Some((0, lines));
                        }
                    }
                }
            }
        }

        if let Some((_, cjk_lines)) = best_cjk_lines {
            return Ok(cjk_lines);
        }
    }

    Ok(default_lines)
}

/// Backwards-compatible helper that runs OCR on a full PNG buffer in `"auto"` mode.
pub fn recognize_image_bytes_sync(png_bytes: &[u8]) -> Result<Vec<OcrLineBox>> {
    recognize_png_with_offset_sync(png_bytes, 0.0, 0.0, 1.0, "auto")
}

#[cfg(not(target_os = "windows"))]
pub fn recognize_cropped_region_sync(
    _frame: &CapturedFrame,
    _rect: ScreenRect,
    _ocr_lang_setting: &str,
) -> Result<Vec<OcrLineBox>> {
    anyhow::bail!("Windows.Media.Ocr requires Windows 10/11")
}

#[cfg(not(target_os = "windows"))]
pub fn recognize_png_with_offset_sync(
    _png_bytes: &[u8],
    _offset_x: f32,
    _offset_y: f32,
    _upscale: f32,
    _ocr_lang_setting: &str,
) -> Result<Vec<OcrLineBox>> {
    anyhow::bail!("Windows.Media.Ocr requires Windows 10/11")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounding_box_intersects() {
        let bbox = BoundingBox {
            x: 100.0,
            y: 100.0,
            width: 200.0,
            height: 30.0,
        };
        let rect_hit = ScreenRect {
            x: 150,
            y: 90,
            width: 100,
            height: 50,
        };
        let rect_miss = ScreenRect {
            x: 400,
            y: 400,
            width: 50,
            height: 50,
        };
        assert!(bbox.intersects(&rect_hit));
        assert!(!bbox.intersects(&rect_miss));
    }

    #[test]
    fn test_clean_cjk_spacing() {
        assert_eq!(
            clean_cjk_spacing("日 本 語 の テ ス ト"),
            "日本語のテスト"
        );
        assert_eq!(
            clean_cjk_spacing("Hello world 日 本 語 test"),
            "Hello world 日本語 test"
        );
    }

    #[test]
    fn test_available_ocr_languages_includes_japanese() {
        let langs = available_ocr_languages();
        println!("Installed Windows.Media.Ocr languages: {:?}", langs);
        assert!(!langs.is_empty());
    }
}
