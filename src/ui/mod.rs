//! GPUI Overlay, Inspector & Preferences module.

pub mod inspector;
pub mod overlay;
pub mod settings;

use crate::capture::{CapturedFrame, ScreenRect};
use crate::config::AppConfig;
use crate::ocr::OcrLineBox;
use crate::services::deepseek::TranslatedLine;
use anyhow::Result;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, ClickEvent, Div, ElementId, FontWeight,
    InteractiveElement, ParentElement, Point, SharedString, Size, Stateful,
    StatefulInteractiveElement as _, Styled, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, div, px, rgb,
};
use gpui_component::{Icon, IconName, Sizable as _};
use overlay::OverlayView;
use settings::SettingsWindowView;

// ============================================================================
// NATIVE MATTE DARK PALETTE (NEUTRAL DESKTOP UTILITY — ZERO NEON / VIBECODE)
// ============================================================================
pub const BG_WINDOW: u32 = 0x181818;
pub const BG_SURFACE: u32 = 0x202020;
pub const BG_INPUT: u32 = 0x141414;
pub const BG_CONTROL: u32 = 0x2a2a2a;
pub const BG_HOVER: u32 = 0x343434;
pub const BG_ACTIVE: u32 = 0x3e3e3e;
pub const BORDER_LINE: u32 = 0x2e2e2e;
pub const BORDER_FOCUS: u32 = 0x505050;
pub const SELECTION_STROKE: u32 = 0xd4d4d4;
pub const TEXT_MAIN: u32 = 0xeaeaea;
pub const TEXT_SUB: u32 = 0x9a9a9a;
pub const TEXT_DIM: u32 = 0x666666;

/// Flat, compact utilitarian toolbar/form button with a subtle 3px radius and neutral contrast.
pub fn tool_btn(
    id: impl Into<ElementId>,
    icon: Option<IconName>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label_str: SharedString = label.into();
    let bg = if active { BG_ACTIVE } else { BG_CONTROL };
    let border = if active { BORDER_FOCUS } else { BORDER_LINE };

    div()
        .id(id)
        .cursor_pointer()
        .px_2p5()
        .py_1()
        .rounded(px(3.0))
        .bg(rgb(bg))
        .border_1()
        .border_color(rgb(border))
        .hover(|h| h.bg(rgb(BG_HOVER)).border_color(rgb(BORDER_FOCUS)))
        .flex()
        .items_center()
        .gap_1p5()
        .when_some(icon, |el, ic| {
            el.child(Icon::new(ic).xsmall().text_color(rgb(TEXT_SUB)))
        })
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(TEXT_MAIN))
                .child(label_str),
        )
        .on_click(on_click)
}

/// Compact icon-only button for tight toolbars.
pub fn icon_btn(
    id: impl Into<ElementId>,
    icon: IconName,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .w(px(26.0))
        .h(px(26.0))
        .rounded(px(3.0))
        .bg(rgb(BG_CONTROL))
        .border_1()
        .border_color(rgb(BORDER_LINE))
        .hover(|h| h.bg(rgb(BG_HOVER)).border_color(rgb(BORDER_FOCUS)))
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).xsmall().text_color(rgb(TEXT_SUB)))
        .on_click(on_click)
}

/// Active interaction mode inside the WinLook overlay after a region is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayMode {
    /// Default mode: auto-extracts OCR inside the selected region.
    RegionSelect,
    /// Painting DeepSeek-translated text lines directly over the OCR bounding boxes inside the selection.
    InPlaceTranslate,
    /// Streaming AI answer below the toolbar.
    AskDeepSeek,
}

/// State held by the GPUI Overlay View.
#[derive(Debug, Clone)]
pub struct OverlaySessionState {
    pub selection: Option<ScreenRect>,
    pub mode: OverlayMode,
    pub ocr_lines: Vec<OcrLineBox>,
    pub ocr_scanning: bool,
    pub auto_ocr_text: String,
    pub show_ocr_panel: bool,
    pub translations: Vec<TranslatedLine>,
    pub ai_prompt: String,
    pub ai_markdown_response: String,
    pub is_loading: bool,
    pub status_message: Option<String>,
}

impl Default for OverlaySessionState {
    fn default() -> Self {
        Self {
            selection: None,
            mode: OverlayMode::RegionSelect,
            ocr_lines: Vec::new(),
            ocr_scanning: false,
            auto_ocr_text: String::new(),
            show_ocr_panel: true,
            translations: Vec::new(),
            ai_prompt: String::new(),
            ai_markdown_response: String::new(),
            is_loading: false,
            status_message: None,
        }
    }
}

/// Opens the borderless topmost fullscreen WinLook overlay window for a captured screen frame.
/// Uses `WindowBounds::Fullscreen` so GPUI places its own window at `(0, 0, width, height)`
/// without any titlebar/frame offset and without ever touching external windows.
pub fn open_overlay_window(
    cx: &mut App,
    frame: CapturedFrame,
    config: AppConfig,
    on_reload_hotkey: impl Fn(String) + 'static + Clone,
) -> Result<AnyWindowHandle> {
    let logical_w = frame.logical_width();
    let logical_h = frame.logical_height();

    let bounds: Bounds<gpui::Pixels> = Bounds::new(
        Point {
            x: px(0.0),
            y: px(0.0),
        },
        Size {
            width: px(logical_w),
            height: px(logical_h),
        },
    );

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Fullscreen(bounds)),
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Opaque,
        window_decorations: None,
        ..Default::default()
    };

    let (handle, _) = gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| OverlayView::new(frame, config, on_reload_hotkey, window, cx))
    })?;

    Ok(handle)
}

/// Restores normal window styles (`WS_OVERLAPPEDWINDOW` and `HWND_NOTOPMOST`) on any visible
/// desktop application window that was accidentally set to `WS_POPUP | WS_VISIBLE` + `WS_EX_TOPMOST`.
pub fn restore_accidentally_locked_windows() {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetWindowLongPtrW, GetWindowTextLengthW,
        GetWindowThreadProcessId, HWND_NOTOPMOST, IsWindowVisible, SWP_FRAMECHANGED,
        SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW,
        WS_POPUP, WS_VISIBLE,
    };

    unsafe extern "system" fn enum_proc(hwnd: HWND, _: LPARAM) -> BOOL {
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                return BOOL(1);
            }

            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == std::process::id() {
                return BOOL(1);
            }

            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let popup_only = WS_POPUP.0 | WS_VISIBLE.0;

            // Only target titled windows that were forced into exact `WS_POPUP | WS_VISIBLE` + `WS_EX_TOPMOST`
            if style == popup_only
                && (ex_style & WS_EX_TOPMOST.0) != 0
                && GetWindowTextLengthW(hwnd) > 0
            {
                let mut cls_buf = [0u16; 128];
                let len = GetClassNameW(hwnd, &mut cls_buf) as usize;
                let cls = String::from_utf16_lossy(&cls_buf[..len]);

                if !matches!(
                    cls.as_str(),
                    "Shell_TrayWnd"
                        | "SecondaryTrayWnd"
                        | "Progman"
                        | "WorkerW"
                        | "Windows.UI.Core.CoreWindow"
                        | "TopLevelWindowForOverflowXamlIsland"
                ) {
                    let restored_style = (WS_OVERLAPPEDWINDOW.0 | WS_VISIBLE.0) as isize;
                    let _ = SetWindowLongPtrW(hwnd, GWL_STYLE, restored_style);
                    let _ = SetWindowPos(
                        hwnd,
                        HWND_NOTOPMOST,
                        80,
                        60,
                        1360,
                        840,
                        SWP_FRAMECHANGED | SWP_SHOWWINDOW,
                    );
                }
            }
            BOOL(1)
        }
    }

    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(0));
    }
}

/// Opens the native-style WinLook Preferences & Capture window.
pub fn open_settings_window(
    cx: &mut App,
    on_trigger_capture: impl Fn(&mut App) + 'static + Clone,
    on_reload_hotkey: impl Fn(String) + 'static + Clone,
) -> Result<AnyWindowHandle> {
    let win_size = Size {
        width: px(580.0),
        height: px(560.0),
    };

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(win_size, cx)),
        titlebar: Some(gpui::TitlebarOptions {
            title: Some("WinLook Preferences".into()),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_movable: true,
        is_resizable: true,
        is_minimizable: true,
        ..Default::default()
    };

    let (handle, _) = gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| SettingsWindowView::new(on_trigger_capture, on_reload_hotkey, window, cx))
    })?;

    Ok(handle)
}
