# WinLook

**WinLook** is a fast, lightweight Windows 10/11 screen capture, OCR, in-place translation, Google Lens visual search, and optional AI vision utility built in **Rust** with **GPUI**.

No Electron, no WebView2, and no 200 MB bundled OCR weights - WinLook sits quietly in the Windows System Tray (~10 MB RAM) and wakes instantaneously when you press your capture shortcut.

---

## Features

- **Instant Region Capture & Screenshot Tool**:
  - Press your global shortcut (default `Alt + Space`, customizable via click-to-record) to freeze the screen and drag a selection rectangle.
  - Copy the cropped image directly to your clipboard (`Ctrl + C`), save to disk (`Ctrl + S`), or take a one-click Fullscreen Screenshot from the tray menu.
- **Native Multi-Language & CJK OCR (`Windows.Media.Ocr`)**:
  - Automatically extracts text inside your selected region in `< 40 ms` completely offline.
  - Supports **Japanese (`ja`)**, Chinese, Korean, English, Spanish, and any installed Windows OCR language pack (`Auto (Multi / Japanese / CJK)` mode automatically detects CJK scripts and removes artificial inter-character spacing).
  - Upscales small region selections 2x for sharp Kanji and small-font recognition.
- **In-Place Screen Translation (No AI Required by Default)**:
  - **Default Engine:** Fast, keyless **Google Translate** - works out-of-the-box with zero configuration or API keys.
  - **Optional AI Engine:** **DeepSeek AI** structured JSON translation when an API key is configured.
  - Renders translated text directly over the original text bounding boxes on screen.
- **One-Click Google Lens Visual Search**:
  - Click **Lens** on any selection to search products, artwork, characters, or sources in Google Lens in your default browser (automatically closing the capture overlay on the same click).
- **Optional DeepSeek Vision & Thinking Mode**:
  - Ask questions about any selected region (code errors, math, tables, charts, UI).
  - Supports `deepseek-flash`, `deepseek-v4-pro`, `deepseek-chat`, and `deepseek-reasoner`.
  - Toggle DeepSeek's native reasoning (`"thinking": { "type": "enabled" | "disabled" }`) directly from the capture toolbar (**`Reason: Off / On`**) or Preferences.
  - All AI buttons and inputs are automatically hidden if you don't configure an API key.

---

## Installation

### Option 1: Windows Installer (`WinLook-Setup.exe`)
Download and run `WinLook-Setup-0.1.0-x64.exe` from the **Releases** page (or build it locally via `scripts\build-release.ps1`).
- Installs per-user into `%LOCALAPPDATA%\Programs\WinLook` (no Administrator UAC prompt required).
- Creates Start Menu and optional Desktop shortcuts.
- Optionally registers WinLook to start silently in the system tray when Windows boots.

### Option 2: Portable Executable
Run `winlook.exe` directly from anywhere - no installation required.

---

## Usage & Shortcuts

Once running, WinLook lives in your **Windows System Tray** (bottom-right taskbar area):
- **Trigger Region Capture:** Press `Alt + Space` (or your custom recorded shortcut).
- **Open Preferences:** Double-click the WinLook tray icon, right-click the tray icon -> **`Preferences...`**, or click the gear icon in the capture overlay.

### Inside the Capture Overlay
| Action | Shortcut / Control |
| :--- | :--- |
| **Select Region** | Left-click and drag |
| **Copy Cropped Image** | Click **Copy Image** or press `Ctrl + C` |
| **Save Cropped PNG** | Click **Save** or press `Ctrl + S` |
| **Google Lens Search** | Click **Lens** (opens browser & closes overlay) |
| **Translate Text In-Place** | Click **Translate** |
| **Copy Single OCR Line** | Click directly on any highlighted text line box |
| **Cancel / Dismiss** | Press `Esc` or right-click anywhere |

---

## Preferences & Security

- **Interactive Keybind Recorder:** Click the shortcut button in Preferences and press any key combination (e.g. `Alt + Space`, `Ctrl + Shift + S`, `Ctrl + Backquote`, `F9`).
- **Start with Windows:** Toggle *"Start WinLook automatically when Windows starts"* (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\WinLook`).
- **Clipboard Controls:**
  - *Auto-copy OCR text to clipboard on region select* (Off by default).
  - *Auto-copy screenshot image to clipboard when saving* (On by default).
- **API Key Security:**
  - DeepSeek API keys are stored **exclusively in the Windows Credential Manager (`wincred.h`)** under service `winlook` / user `deepseek_api_key` (never in plaintext config files).
  - Non-sensitive preferences are saved in `%APPDATA%\WinLook\config.json`.

---

## Building from Source

### Prerequisites (Windows 10 / 11)
1. **Rust Toolchain (`1.95.0` MSVC)**:
   Pinned automatically via [`rust-toolchain.toml`](./rust-toolchain.toml):
   ```powershell
   rustup toolchain install 1.95.0-x86_64-pc-windows-msvc
   ```
2. **Visual Studio 2022 Build Tools**:
   Install the **"Desktop development with C++"** workload (Windows 10/11 SDK).
3. **(Optional) Inno Setup 6** (to build `WinLook-Setup.exe`):
   ```powershell
   winget install JRSoftware.InnoSetup
   ```
4. **(Optional) Japanese / CJK Windows OCR Language Packs**:
   Install additional OCR languages under *Windows Settings -> Time & language -> Language & region -> Add a language*.

### Development & Testing
```powershell
# Build and run in development mode (optimized with opt-level = 2)
cargo run

# Run the test suite
cargo test -- --nocapture
```

### Build Release Binary & Installer
```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build-release.ps1
```
Outputs:
- **Release Binary:** `target\release\winlook.exe`
- **Windows Installer:** `target\installer\WinLook-Setup-0.1.0-x64.exe`

---

## Documentation

- **[Architecture & Technical Design](./docs/ARCHITECTURE.md)** - Capture pipeline, `WindowBounds::Fullscreen` DPI handling, and multi-language `Windows.Media.Ocr` architecture.
- **[Translation, DeepSeek AI & Google Lens](./docs/AI_INTEGRATION.md)** - Free Google Translate (`gtx`), DeepSeek `thinking` mode parameter, Windows Credential Manager, and Google Lens upload flow.

---

## License

Licensed under the [MIT License](./LICENSE).
