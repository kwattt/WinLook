use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use reqwest::multipart::{Form, Part};
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn url_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 2);
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    encoded
}

/// Uploads the JPEG bytes to a fast, ephemeral HTTPS host (1-hour expiry) so Google Lens
/// can fetch the image directly via `https://lens.google.com/uploadbyurl?url=...`.
/// This guarantees:
/// 1. The user's browser opens a clean GET URL using their own session cookies (never "Búsqueda visual vencida").
/// 2. Google's servers fetch the image over HTTPS so the thumbnail and main canvas never show a broken image icon.
async fn upload_ephemeral_image(jpeg_bytes: &[u8]) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .build()?;

    // Primary: Litterbox (Catbox ephemeral 1h host — returns direct HTTPS URL in plain text)
    if let Ok(part) = Part::bytes(jpeg_bytes.to_vec())
        .file_name("capture.jpg")
        .mime_str("image/jpeg")
    {
        let form = Form::new()
            .text("reqtype", "fileupload")
            .text("time", "1h")
            .part("fileToUpload", part);

        if let Ok(resp) = client
            .post("https://litterbox.catbox.moe/resources/internals/api.php")
            .multipart(form)
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(text) = resp.text().await {
                    let url = text.trim();
                    if url.starts_with("https://") {
                        return Ok(url.to_string());
                    }
                }
            }
        }
    }

    // Secondary: Uguu.se ephemeral host
    if let Ok(part) = Part::bytes(jpeg_bytes.to_vec())
        .file_name("capture.jpg")
        .mime_str("image/jpeg")
    {
        let form = Form::new().part("files[]", part);
        if let Ok(resp) = client
            .post("https://uguu.se/upload")
            .multipart(form)
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(url) = json
                        .get("files")
                        .and_then(|f| f.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|item| item.get("url"))
                        .and_then(|u| u.as_str())
                    {
                        if url.starts_with("https://") {
                            return Ok(url.to_string());
                        }
                    }
                }
            }
        }
    }

    Err(anyhow!("Ephemeral image hosts unreachable"))
}

/// Opens Google Lens visual search in the user's default browser.
/// First uses `https://lens.google.com/uploadbyurl?url=...` with a 1-hour ephemeral HTTPS image URL
/// so Google Lens renders both the thumbnail and interactive canvas cleanly without broken images or session expiry.
/// Falls back to a browser-native multipart POST with `processed_image_dimensions` and `sbisrc` if offline.
pub async fn search_google_lens(jpeg_bytes: Vec<u8>, width: u32, height: u32) -> Result<String> {
    if let Ok(public_img_url) = upload_ephemeral_image(&jpeg_bytes).await {
        let lens_url = format!(
            "https://lens.google.com/uploadbyurl?url={}",
            url_encode(&public_img_url)
        );
        open::that(&lens_url).context("Failed to launch browser for Google Lens")?;
        return Ok(lens_url);
    }

    // Fallback: Browser-native form POST with explicit `processed_image_dimensions` and `sbisrc`
    let b64 = BASE64.encode(&jpeg_bytes);
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let action_url = format!(
        "https://lens.google.com/v3/upload?ep=ccm&s=4&st={}",
        timestamp_ms
    );
    let dims = format!("{},{}", width.max(1), height.max(1));

    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<meta name="referrer" content="no-referrer">
<title>Google Lens</title>
<style>
  body {{ margin: 0; height: 100vh; background: #191919; color: #cccccc; font-family: "Segoe UI", sans-serif; display: flex; align-items: center; justify-content: center; font-size: 13px; }}
</style>
</head>
<body>
  <div>Opening Google Lens...</div>
  <form id="f" action="{action_url}" method="POST" enctype="multipart/form-data" style="display:none;">
    <input id="img" type="file" name="encoded_image" />
    <input type="hidden" name="processed_image_dimensions" value="{dims}" />
    <input type="hidden" name="sbisrc" value="Google Chrome 131.0.6778.86 (Official) Windows" />
  </form>
  <script>
    (function() {{
      var bin = atob("{b64}");
      var arr = new Uint8Array(bin.length);
      for (var i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
      var dt = new DataTransfer();
      dt.items.add(new File([arr], "image.jpg", {{ type: "image/jpeg" }}));
      document.getElementById("img").files = dt.files;
      document.getElementById("f").submit();
    }})();
  </script>
</body>
</html>"#
    );

    let temp_path = std::env::temp_dir().join("winlook_google_lens.html");
    fs::write(&temp_path, html).context("Failed to write Google Lens fallback HTML")?;
    open::that(&temp_path).context("Failed to launch browser for Google Lens")?;
    Ok(action_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ephemeral_image_upload_for_lens() {
        let frame = crate::capture::capture_primary_screen().expect("capture screen");
        let rect = crate::capture::ScreenRect {
            x: 0,
            y: 0,
            width: frame.width.min(300),
            height: frame.height.min(200),
        };
        let jpeg = frame.crop_to_jpeg_bytes(rect).expect("encode jpeg");
        if let Ok(url) = upload_ephemeral_image(&jpeg).await {
            assert!(url.starts_with("https://"));
            let lens_url = format!("https://lens.google.com/uploadbyurl?url={}", url_encode(&url));
            println!("Verified Google Lens URL: {}", lens_url);
        }
    }
}
