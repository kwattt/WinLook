use anyhow::{Context, Result};
use global_hotkey::hotkey::HotKey;
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;

const KEYRING_SERVICE: &str = "winlook";
const KEYRING_USER_DEEPSEEK: &str = "deepseek_api_key";
const RUN_REGISTRY_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_REGISTRY_VALUE: &str = "WinLook";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const SUPPORTED_LANGUAGES: &[&str] = &[
    "English",
    "Spanish",
    "Chinese (Simplified)",
    "Japanese",
    "Korean",
    "German",
    "French",
    "Russian",
    "Portuguese",
];

pub const HOTKEY_PRESETS: &[&str] = &[
    "Alt+Space",
    "Alt+Shift+S",
    "Alt+Shift+A",
    "Ctrl+Shift+Space",
    "Ctrl+Shift+X",
    "F9",
];

/// DeepSeek API models per https://api-docs.deepseek.com/quick_start/pricing
pub const DEEPSEEK_MODELS: &[&str] = &[
    "deepseek-flash",
    "deepseek-v4-pro",
    "deepseek-chat",
    "deepseek-reasoner",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Global hotkey combination string, e.g. "Alt+Space"
    pub hotkey: String,
    /// OCR recognition language tag (`"auto"` for automatic multi-script/CJK detection, or a specific BCP-47 tag like `"ja"`, `"en-US"`)
    pub ocr_language: String,
    /// Target language for in-place overlay translation
    pub target_language: String,
    /// Whether to use DeepSeek AI for translation when an API key is configured (false = fast free Google Translate)
    pub use_ai_translation: bool,
    /// DeepSeek API base URL (`https://api.deepseek.com`)
    pub deepseek_base_url: String,
    /// DeepSeek model identifier (`deepseek-flash`, `deepseek-v4-pro`, `deepseek-chat`, `deepseek-reasoner`)
    pub deepseek_model: String,
    /// Whether AI Reasoning / Thinking mode (`"thinking": {"type": "enabled" | "disabled"}`) is enabled (default: false)
    pub enable_ai_reasoning: bool,
    /// Directory where screenshots are saved
    pub screenshot_dir: String,
    /// Automatically copy screenshot to clipboard when saving
    pub auto_copy_on_save: bool,
    /// Automatically copy recognized OCR text to clipboard immediately upon selecting a region (default: false)
    pub auto_copy_ocr: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        let default_shots = Self::default_screenshots_dir()
            .to_string_lossy()
            .to_string();
        Self {
            hotkey: "Alt+Space".to_string(),
            ocr_language: "auto".to_string(),
            target_language: "Spanish".to_string(),
            use_ai_translation: false,
            deepseek_base_url: "https://api.deepseek.com".to_string(),
            deepseek_model: "deepseek-flash".to_string(),
            enable_ai_reasoning: false,
            screenshot_dir: default_shots,
            auto_copy_on_save: true,
            auto_copy_ocr: false,
        }
    }
}

impl AppConfig {
    /// Returns the default `Pictures\WinLook` folder on Windows.
    pub fn default_screenshots_dir() -> PathBuf {
        if let Some(user_profile) = std::env::var_os("USERPROFILE") {
            PathBuf::from(user_profile).join("Pictures").join("WinLook")
        } else {
            std::env::temp_dir().join("WinLook")
        }
    }

    /// Returns the resolved screenshot directory, creating it if needed.
    pub fn ensure_screenshots_dir(&self) -> Result<PathBuf> {
        let dir = if self.screenshot_dir.trim().is_empty() {
            Self::default_screenshots_dir()
        } else {
            PathBuf::from(self.screenshot_dir.trim())
        };
        fs::create_dir_all(&dir).context("Failed to create screenshots directory")?;
        Ok(dir)
    }

    /// Opens the native Windows Folder Browser Dialog and returns the selected folder path.
    pub fn pick_folder_dialog(initial_dir: &str) -> Option<String> {
        let escaped = initial_dir.replace('\'', "''");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
             $d.Description = 'Select Screenshot Save Folder'; \
             $d.SelectedPath = '{}'; \
             if ($d.ShowDialog() -eq 'OK') {{ [Console]::Out.Write($d.SelectedPath) }}",
            escaped
        );
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-STA",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &script,
            ])
            .output()
            .ok()?;
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() { None } else { Some(path) }
    }

    /// Returns the path to `%APPDATA%\WinLook\config.json` on Windows.
    pub fn config_path() -> Option<PathBuf> {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|dir| dir.join("WinLook").join("config.json"))
    }

    /// Loads `AppConfig` from `%APPDATA%\WinLook\config.json`, falling back to `Default`.
    /// Automatically migrates and scrubs any legacy plaintext `deepseek_api_key` into Windows Credential Manager.
    pub fn load() -> Self {
        if let Some(path) = Self::config_path() {
            if let Ok(contents) = fs::read_to_string(&path) {
                if let Ok(raw_val) = serde_json::from_str::<serde_json::Value>(&contents) {
                    if let Some(legacy_key) = raw_val
                        .get("deepseek_api_key")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER_DEEPSEEK)
                        {
                            let _ = entry.set_password(legacy_key);
                        }
                        if let Ok(cfg) = serde_json::from_value::<AppConfig>(raw_val) {
                            let _ = cfg.save();
                            return cfg;
                        }
                    } else if let Ok(cfg) = serde_json::from_value::<AppConfig>(raw_val) {
                        return cfg;
                    }
                }
            }
        }
        Self::default()
    }

    /// Saves `AppConfig` to `%APPDATA%\WinLook\config.json`.
    pub fn save(&self) -> Result<()> {
        if let Some(path) = Self::config_path() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .context("Failed to create WinLook config directory in APPDATA")?;
            }
            let json = serde_json::to_string_pretty(self)?;
            fs::write(&path, json).context("Failed to write config.json")?;
        }
        Ok(())
    }

    /// Cycles to the next installed Windows OCR recognizer language (`"auto"` -> installed languages).
    pub fn next_ocr_language(&mut self) -> String {
        let langs = crate::ocr::available_ocr_languages();
        let idx = langs
            .iter()
            .position(|l| l.eq_ignore_ascii_case(&self.ocr_language))
            .unwrap_or(0);
        let next_idx = (idx + 1) % langs.len();
        self.ocr_language = langs[next_idx].clone();
        let _ = self.save();
        self.ocr_language.clone()
    }

    /// Checks whether WinLook is registered in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
    pub fn is_autostart_enabled() -> bool {
        Command::new("reg")
            .args(["query", RUN_REGISTRY_KEY, "/v", RUN_REGISTRY_VALUE])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Enables or disables starting WinLook silently in the background when Windows loads.
    pub fn set_autostart_enabled(enabled: bool) -> Result<()> {
        if enabled {
            let exe = std::env::current_exe().context("Failed to resolve current executable path")?;
            let cmd_val = format!("\"{}\" --silent", exe.display());
            let status = Command::new("reg")
                .args([
                    "add",
                    RUN_REGISTRY_KEY,
                    "/v",
                    RUN_REGISTRY_VALUE,
                    "/t",
                    "REG_SZ",
                    "/d",
                    &cmd_val,
                    "/f",
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .context("Failed to execute reg add for Windows startup")?;
            if !status.success() {
                anyhow::bail!("Registry update failed with status: {}", status);
            }
        } else {
            let _ = Command::new("reg")
                .args(["delete", RUN_REGISTRY_KEY, "/v", RUN_REGISTRY_VALUE, "/f"])
                .creation_flags(CREATE_NO_WINDOW)
                .status();
        }
        Ok(())
    }

    /// Cycles to the next supported target language.
    pub fn next_target_language(&mut self) -> &str {
        let idx = SUPPORTED_LANGUAGES
            .iter()
            .position(|&lang| lang.eq_ignore_ascii_case(&self.target_language))
            .unwrap_or(0);
        let next_idx = (idx + 1) % SUPPORTED_LANGUAGES.len();
        self.target_language = SUPPORTED_LANGUAGES[next_idx].to_string();
        let _ = self.save();
        &self.target_language
    }

    /// Cycles to the next DeepSeek API model.
    pub fn next_deepseek_model(&mut self) -> &str {
        let idx = DEEPSEEK_MODELS
            .iter()
            .position(|&m| m.eq_ignore_ascii_case(&self.deepseek_model))
            .unwrap_or(0);
        let next_idx = (idx + 1) % DEEPSEEK_MODELS.len();
        self.deepseek_model = DEEPSEEK_MODELS[next_idx].to_string();
        let _ = self.save();
        &self.deepseek_model
    }

    /// Checks whether a DeepSeek API key is available in config.json, Windows Keyring, or environment.
    pub fn has_deepseek_api_key() -> bool {
        Self::get_deepseek_api_key().is_ok()
    }

    /// Returns a masked preview (e.g. `sk-••••abcd`) if an API key is configured.
    pub fn masked_api_key() -> Option<String> {
        let key = Self::get_deepseek_api_key().ok()?;
        let chars: Vec<char> = key.chars().collect();
        if chars.len() > 7 {
            let prefix: String = chars[..3].iter().collect();
            let suffix: String = chars[chars.len() - 4..].iter().collect();
            Some(format!("{}••••{}", prefix, suffix))
        } else {
            Some("••••••••".to_string())
        }
    }

    /// Retrieves the DeepSeek API key from Windows Credential Manager (`keyring` with `windows-native`)
    /// or `DEEPSEEK_API_KEY` environment variable.
    pub fn get_deepseek_api_key() -> Result<String> {
        // Ensure any legacy plaintext key in config.json is migrated into Windows Credential Manager first
        let _ = Self::load();

        // 1. Check Windows Credential Manager (`CredReadW`)
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER_DEEPSEEK) {
            if let Ok(key) = entry.get_password() {
                if !key.trim().is_empty() {
                    return Ok(key.trim().to_string());
                }
            }
        }

        // 2. Check environment variable
        if let Ok(env_key) = std::env::var("DEEPSEEK_API_KEY") {
            if !env_key.trim().is_empty() {
                return Ok(env_key.trim().to_string());
            }
        }

        anyhow::bail!("DeepSeek API key not configured. Open Preferences to save your key.")
    }

    /// Persists the DeepSeek API key securely into Windows Credential Manager (`keyring` `CredWriteW`).
    pub fn set_deepseek_api_key(api_key: &str) -> Result<()> {
        let trimmed = api_key.trim();
        if trimmed.is_empty() {
            anyhow::bail!("API key cannot be empty");
        }

        let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER_DEEPSEEK)
            .context("Failed to open Windows Credential Manager entry")?;
        entry
            .set_password(trimmed)
            .context("Failed to write API key to Windows Credential Manager")?;

        Ok(())
    }
}

/// Converts a GPUI keystroke (modifiers + key name) into a valid `global-hotkey` string.
/// Returns `None` if the pressed key is only a bare modifier key (e.g. just pressing Ctrl/Alt/Shift alone).
pub fn format_recorded_hotkey(
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
    raw_key: &str,
) -> Option<String> {
    let k = raw_key.trim().to_ascii_lowercase();

    // Ignore bare modifier presses while waiting for the actual key
    if matches!(
        k.as_str(),
        "" | "control" | "ctrl" | "alt" | "shift" | "win" | "super" | "meta" | "command"
    ) {
        return None;
    }

    let key_token = match k.as_str() {
        " " | "space" => "Space".to_string(),
        "tab" => "Tab".to_string(),
        "enter" | "return" => "Enter".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        "insert" => "Insert".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" => "PageUp".to_string(),
        "pagedown" => "PageDown".to_string(),
        "up" | "arrowup" => "ArrowUp".to_string(),
        "down" | "arrowdown" => "ArrowDown".to_string(),
        "left" | "arrowleft" => "ArrowLeft".to_string(),
        "right" | "arrowright" => "ArrowRight".to_string(),
        "`" | "backquote" => "Backquote".to_string(),
        "-" | "minus" => "Minus".to_string(),
        "=" | "equal" => "Equal".to_string(),
        "[" => "BracketLeft".to_string(),
        "]" => "BracketRight".to_string(),
        "\\" | "backslash" => "Backslash".to_string(),
        ";" | "semicolon" => "Semicolon".to_string(),
        "'" | "quote" => "Quote".to_string(),
        "," | "comma" => "Comma".to_string(),
        "." | "period" => "Period".to_string(),
        "/" | "slash" => "Slash".to_string(),
        s if s.len() == 1 && s.chars().next().unwrap().is_ascii_alphabetic() => {
            s.to_ascii_uppercase()
        }
        s if s.len() == 1 && s.chars().next().unwrap().is_ascii_digit() => {
            format!("Digit{}", s)
        }
        s if s.starts_with('f') && s[1..].parse::<u8>().is_ok() => s.to_ascii_uppercase(),
        _ => return None,
    };

    // Require at least one modifier unless it's a Function key (F1-F24)
    let is_function_key = key_token.starts_with('F') && key_token[1..].parse::<u8>().is_ok();
    if !ctrl && !alt && !shift && !win && !is_function_key {
        return None;
    }

    let mut parts = Vec::new();
    if ctrl {
        parts.push("Ctrl".to_string());
    }
    if alt {
        parts.push("Alt".to_string());
    }
    if shift {
        parts.push("Shift".to_string());
    }
    if win {
        parts.push("Super".to_string());
    }
    parts.push(key_token);

    let combo = parts.join("+");
    if HotKey::from_str(&combo).is_ok() {
        Some(combo)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_recorded_hotkey() {
        assert_eq!(
            format_recorded_hotkey(false, true, false, false, "space"),
            Some("Alt+Space".to_string())
        );
        assert_eq!(
            format_recorded_hotkey(true, false, true, false, "s"),
            Some("Ctrl+Shift+S".to_string())
        );
        assert_eq!(
            format_recorded_hotkey(false, false, false, false, "f9"),
            Some("F9".to_string())
        );
        // Bare key without modifier should not trigger unless F-key
        assert_eq!(format_recorded_hotkey(false, false, false, false, "a"), None);
    }

    #[test]
    fn test_windows_credential_manager_and_no_plaintext_in_config_json() {
        // 1. Trigger AppConfig::load() which migrates any legacy plaintext key into Windows Credential Manager
        let cfg = AppConfig::load();
        let _ = cfg.save();

        // 2. Verify config.json on disk does NOT contain "deepseek_api_key"
        if let Some(path) = AppConfig::config_path() {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                assert!(
                    !contents.contains("deepseek_api_key"),
                    "config.json must never contain deepseek_api_key"
                );
            }
        }

        // 3. Verify Windows Credential Manager round-trip via a dedicated test entry
        let entry = keyring::Entry::new("winlook_test", "credential_manager_check")
            .expect("keyring Entry::new failed");
        entry
            .set_password("sk-test-wincred-persistence")
            .expect("Windows Credential Manager set_password failed");
        let loaded = entry
            .get_password()
            .expect("Windows Credential Manager get_password failed");
        assert_eq!(loaded, "sk-test-wincred-persistence");
        let _ = entry.delete_credential();
    }
}
