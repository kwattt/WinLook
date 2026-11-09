use crate::ocr::OcrLineBox;
use crate::services::deepseek::TranslatedLine;
use anyhow::{Context, Result};
use reqwest::Client;
use std::time::Duration;

/// Maps human-readable language names from `AppConfig` to ISO-639-1 codes for Google Translate GTX.
pub fn language_to_iso_code(lang: &str) -> &'static str {
    match lang.trim().to_ascii_lowercase().as_str() {
        "english" => "en",
        "spanish" => "es",
        "chinese (simplified)" | "chinese" => "zh-CN",
        "japanese" => "ja",
        "korean" => "ko",
        "german" => "de",
        "french" => "fr",
        "russian" => "ru",
        "portuguese" => "pt",
        _ => "en",
    }
}

/// Translates a single text string using the keyless Google Translate `gtx` endpoint.
async fn translate_text_gtx(client: &Client, text: &str, target_code: &str) -> Result<String> {
    let resp: serde_json::Value = client
        .get("https://translate.googleapis.com/translate_a/single")
        .query(&[
            ("client", "gtx"),
            ("sl", "auto"),
            ("tl", target_code),
            ("dt", "t"),
            ("q", text),
        ])
        .send()
        .await
        .context("Failed to reach Google Translate endpoint")?
        .error_for_status()
        .context("Google Translate returned an error status")?
        .json()
        .await
        .context("Failed to parse Google Translate JSON")?;

    let mut out = String::new();
    if let Some(sentences) = resp.get(0).and_then(|v| v.as_array()) {
        for item in sentences {
            if let Some(chunk) = item.get(0).and_then(|s| s.as_str()) {
                out.push_str(chunk);
            }
        }
    }

    Ok(out)
}

/// Translates OCR bounding-box lines without any AI API key using Google Translate's fast GTX endpoint.
/// Preserves 1-to-1 line alignment with `OcrLineBox::id` for in-place screen overlay rendering.
pub async fn translate_ocr_lines_free(
    lines: &[OcrLineBox],
    target_language: &str,
) -> Result<Vec<TranslatedLine>> {
    if lines.is_empty() {
        return Ok(Vec::new());
    }

    let target_code = language_to_iso_code(target_language);
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .build()?;

    // Fast path: join all lines with newline so the entire region translates in one HTTP round-trip
    let joined: String = lines
        .iter()
        .map(|l| l.text.replace('\n', " "))
        .collect::<Vec<_>>()
        .join("\n");

    if let Ok(batch_translated) = translate_text_gtx(&client, &joined, target_code).await {
        let split_lines: Vec<&str> = batch_translated.split('\n').collect();
        if split_lines.len() == lines.len() {
            return Ok(lines
                .iter()
                .zip(split_lines.into_iter())
                .map(|(orig, trans)| TranslatedLine {
                    id: orig.id,
                    text: trans.trim().to_string(),
                })
                .collect());
        }
    }

    // Fallback: translate each line individually if newline count shifted
    let mut results = Vec::with_capacity(lines.len());
    for line in lines {
        let translated = translate_text_gtx(&client, &line.text, target_code)
            .await
            .unwrap_or_else(|_| line.text.clone());
        results.push(TranslatedLine {
            id: line.id,
            text: translated.trim().to_string(),
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::BoundingBox;

    #[tokio::test]
    async fn test_free_translation_without_ai() {
        let sample = vec![
            OcrLineBox {
                id: 0,
                text: "Hello world".to_string(),
                bounds: BoundingBox {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height: 20.0,
                },
                words: Vec::new(),
            },
            OcrLineBox {
                id: 1,
                text: "Settings and Preferences".to_string(),
                bounds: BoundingBox {
                    x: 0.0,
                    y: 25.0,
                    width: 150.0,
                    height: 20.0,
                },
                words: Vec::new(),
            },
        ];

        let translated = translate_ocr_lines_free(&sample, "Spanish")
            .await
            .expect("free translation should succeed");
        assert_eq!(translated.len(), 2);
        assert_eq!(translated[0].id, 0);
        assert!(!translated[0].text.is_empty());
        println!("Free translation result: {:?}", translated);
    }
}
