# WinLook — Translation, DeepSeek AI & Google Lens Integration

## 1. Non-AI Translation (Default Engine: Google Translate `gtx`)

WinLook translates screen text **without requiring any AI API key** by default (`use_ai_translation: false`).

### How `src/services/translator.rs` Works
1. Joins recognized `OcrLineBox` lines inside the selected region with newline delimiters.
2. Sends an HTTPS GET request to Google Translate's keyless `gtx` endpoint:
   ```text
   https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl=<lang_code>&dt=t&q=<encoded_text>
   ```
3. Parses the nested JSON sentence segments `response[0][*][0]` and maps each translated line back to its original `OcrLineBox.id` so the translation is painted directly over the original text box on screen.
4. Also serves as an automatic fallback if DeepSeek AI translation is enabled but fails or times out.

---

## 2. DeepSeek API & Thinking Mode (`https://api-docs.deepseek.com/guides/thinking_mode`)

When a DeepSeek API key is configured, WinLook enables multimodal/text queries and optional AI translation via `https://api.deepseek.com/chat/completions`.

### Credential Security (Windows Credential Manager)
- **Storage:** Stored exclusively in the native **Windows Credential Manager (`wincred.h`)** via `keyring` (`features = ["windows-native"]`, service `"winlook"`, username `"deepseek_api_key"`), with fallback to the `DEEPSEEK_API_KEY` environment variable.
- **No Plaintext in `config.json`:** `AppConfig` never serializes the API key to disk, and `AppConfig::load()` automatically migrates and scrubs any legacy key from `%APPDATA%\WinLook\config.json`.

### Explicit `thinking` Parameter Control
Per the official [DeepSeek Thinking Mode Guide](https://api-docs.deepseek.com/guides/thinking_mode), WinLook **never switches or overrides your selected model** (`deepseek-flash`, `deepseek-v4-pro`, `deepseek-chat`, `deepseek-reasoner`). Instead, it passes the top-level `"thinking"` object directly in the request payload:

- **Thinking Disabled (Default (`enable_ai_reasoning: false`) & JSON Translation):**
  ```json
  {
    "model": "deepseek-flash",
    "stream": true,
    "thinking": {
      "type": "disabled"
    },
    "messages": [...]
  }
  ```
- **Thinking Enabled (`enable_ai_reasoning: true`):**
  ```json
  {
    "model": "deepseek-flash",
    "stream": true,
    "thinking": {
      "type": "enabled"
    },
    "messages": [...]
  }
  ```
  When `"type": "enabled"` is active, `DeepSeekClient::ask_vision_stream` streams `delta.reasoning_content` (`> *Thinking:* ...`) followed by `delta.content` in real time.

---

## 3. Google Lens Visual Search (`src/services/lens.rs`)

When the user clicks **Lens** on the selection toolbar:
1. Theoverlay window closes immediately (`window.remove_window()`) on the same click while the background Tokio runtime completes the upload.
2. The selected screen region is encoded as a high-quality JPEG (`quality = 92`).
3. To ensure Google Lens renders its interactive image cropper canvas without a broken image preview, `search_google_lens` uploads the JPEG to a 1-hour auto-expiring HTTPS host (`https://litterbox.catbox.moe/resources/internals/api.php` with `time=1h`, falling back to `https://uguu.se/upload`) and opens:
   ```text
   https://lens.google.com/uploadbyurl?url=<encoded_https_image_url>
   ```
   in the default browser (with a direct multipart POST to `https://lens.google.com/v3/upload` as a secondary fallback).
