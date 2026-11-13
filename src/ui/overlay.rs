use crate::capture::{CapturedFrame, ScreenRect, copy_text_to_clipboard};
use crate::config::AppConfig;
use crate::ocr::{self, OcrLineBox};
use crate::services::deepseek::{DeepSeekClient, TranslatedLine};
use crate::services::lens::search_google_lens;
use crate::ui::inspector::render_floating_inspector;
use crate::ui::{
    BG_SURFACE, BG_WINDOW, BORDER_FOCUS, BORDER_LINE, OverlayMode, OverlaySessionState,
    SELECTION_STROKE, TEXT_MAIN, TEXT_SUB, icon_btn, open_overlay_window, open_settings_window,
    tool_btn,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, CursorStyle, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, ParentElement, Render, SharedString, Styled, StyledImage as _,
    Subscription, Window, div, img, px, rgb, rgba,
};
use gpui_component::IconName;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{h_flex, v_flex};
use std::rc::Rc;
use std::sync::OnceLock;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

gpui_kit::actions!(
    winlook_overlay,
    [DismissOverlay, CopySelection, SaveSelectionPng]
);

/// Shared background Tokio runtime for `reqwest` (DeepSeek SSE + Google Lens) and `Windows.Media.Ocr`.
pub fn tokio_runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Failed to initialize WinLook Tokio runtime")
    })
}

enum StreamUpdate {
    Delta(String),
    Done,
    Error(String),
}

pub struct OverlayView {
    pub frame: CapturedFrame,
    pub config: AppConfig,
    pub state: OverlaySessionState,
    pub focus_handle: FocusHandle,
    pub prompt_input: Entity<InputState>,
    pub api_key_input: Entity<InputState>,
    pub show_api_key_modal: bool,
    pub on_reload_hotkey: Rc<dyn Fn(String)>,
    drag_start: Option<(f32, f32)>,
    is_dragging: bool,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for OverlayView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl OverlayView {
    pub fn new(
        frame: CapturedFrame,
        config: AppConfig,
        on_reload_hotkey: impl Fn(String) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.bind_keys([
            KeyBinding::new("escape", DismissOverlay, Some("WinLookOverlay")),
            KeyBinding::new("ctrl-c", CopySelection, Some("WinLookOverlay")),
            KeyBinding::new("ctrl-s", SaveSelectionPng, Some("WinLookOverlay")),
        ]);

        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);

        let prompt_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Ask DeepSeek about selection...")
        });

        let api_key_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Paste DeepSeek API Key (sk-...)")
        });

        let sub_prompt = cx.subscribe_in(
            &prompt_input,
            window,
            |this: &mut Self, state, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let text = state.read(cx).value().to_string();
                    if !text.trim().is_empty() {
                        this.trigger_ask_deepseek(text, window, cx);
                    }
                }
            },
        );

        let sub_api_key = cx.subscribe_in(
            &api_key_input,
            window,
            |this: &mut Self, _state, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.save_api_key_from_input(window, cx);
                }
            },
        );

        Self {
            frame,
            config,
            state: OverlaySessionState::default(),
            focus_handle,
            prompt_input,
            api_key_input,
            show_api_key_modal: false,
            on_reload_hotkey: Rc::new(on_reload_hotkey),
            drag_start: None,
            is_dragging: false,
            _subscriptions: vec![sub_prompt, sub_api_key],
        }
    }

    /// Runs fast cropped-region multi-language OCR (`auto` / `ja` / CJK / Latin) on the selected rectangle,
    /// populates `ocr_lines` & `auto_ocr_text`, and copies recognized text to the clipboard.
    pub fn run_auto_ocr_for_selection(&mut self, cx: &mut Context<Self>) {
        let Some(rect) = self.state.selection else {
            self.state.auto_ocr_text.clear();
            return;
        };

        self.state.ocr_scanning = true;
        self.state.ocr_lines.clear();
        self.state.auto_ocr_text.clear();
        self.state.translations.clear();
        self.state.status_message = Some("Recognizing text...".to_string());

        let frame = self.frame.clone();
        let ocr_lang = self.config.ocr_language.clone();
        let (tx, mut rx) = mpsc::unbounded_channel::<anyhow::Result<Vec<OcrLineBox>>>();

        tokio_runtime().spawn_blocking(move || {
            let res = ocr::recognize_cropped_region_sync(&frame, rect, &ocr_lang);
            let _ = tx.send(res);
        });

        cx.spawn(async move |this, cx| {
            if let Some(result) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.state.ocr_scanning = false;
                    if this.state.selection != Some(rect) {
                        return;
                    }
                    match result {
                        Ok(lines) => {
                            let extracted = ocr::extract_text_in_rect(&lines, &rect);
                            this.state.ocr_lines = lines;
                            this.state.auto_ocr_text = extracted.clone();

                            if !extracted.trim().is_empty() {
                                this.state.show_ocr_panel = true;
                                let count = extracted.lines().count();
                                if this.config.auto_copy_ocr {
                                    let _ = copy_text_to_clipboard(&extracted);
                                    this.state.status_message = Some(format!(
                                        "OCR: {} {} copied to clipboard",
                                        count,
                                        if count == 1 { "line" } else { "lines" }
                                    ));
                                } else {
                                    this.state.status_message = None;
                                }
                            } else {
                                this.state.show_ocr_panel = false;
                                this.state.status_message = None;
                            }
                        }
                        Err(e) => {
                            this.state.show_ocr_panel = false;
                            this.state.status_message = Some(format!("OCR error: {}", e));
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn effective_selection(&self) -> ScreenRect {
        self.state
            .selection
            .unwrap_or_else(|| self.frame.full_rect())
    }

    pub fn copy_selected_text(&mut self, cx: &mut Context<Self>) {
        let rect = self.effective_selection();
        let text = ocr::extract_text_in_rect(&self.state.ocr_lines, &rect);

        if text.trim().is_empty() {
            self.copy_selected_image(cx);
            return;
        }

        if copy_text_to_clipboard(&text).is_ok() {
            let line_count = text.lines().count();
            self.state.status_message = Some(format!(
                "Copied {} {} to clipboard",
                line_count,
                if line_count == 1 { "line" } else { "lines" }
            ));
        }
        cx.notify();
    }

    pub fn copy_selected_image(&mut self, cx: &mut Context<Self>) {
        let rect = self.effective_selection();
        match self.frame.copy_crop_to_clipboard(rect) {
            Ok(_) => {
                self.state.status_message = Some("Copied image to clipboard".to_string());
            }
            Err(e) => {
                self.state.status_message = Some(format!("Clipboard error: {}", e));
            }
        }
        cx.notify();
    }

    pub fn save_selected_screenshot(&mut self, cx: &mut Context<Self>) {
        let rect = self.effective_selection();
        match self.frame.save_crop_to_disk(rect, &self.config) {
            Ok(path) => {
                let filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("screenshot.png");
                self.state.status_message = Some(format!("Saved {}", filename));
            }
            Err(e) => {
                self.state.status_message = Some(format!("Save error: {}", e));
            }
        }
        cx.notify();
    }

    pub fn trigger_in_place_translation(
        &mut self,
        rect_override: Option<ScreenRect>,
        cx: &mut Context<Self>,
    ) {
        let target_rect = rect_override.unwrap_or_else(|| self.effective_selection());
        let existing_lines = ocr::lines_in_rect(&self.state.ocr_lines, &target_rect);
        let use_ai = self.config.use_ai_translation && AppConfig::has_deepseek_api_key();

        self.state.mode = OverlayMode::InPlaceTranslate;
        self.state.is_loading = true;
        self.state.status_message = Some(format!(
            "Translating to {}{}...",
            self.config.target_language,
            if use_ai { " (AI)" } else { "" }
        ));
        cx.notify();

        let client = DeepSeekClient::new(self.config.clone());
        let target_lang = self.config.target_language.clone();
        let ocr_lang = self.config.ocr_language.clone();
        let frame = self.frame.clone();
        let (tx, mut rx) =
            mpsc::unbounded_channel::<anyhow::Result<(Vec<OcrLineBox>, Vec<TranslatedLine>)>>();

        tokio_runtime().spawn(async move {
            let lines = if !existing_lines.is_empty() {
                existing_lines
            } else {
                let f = frame.clone();
                let ol = ocr_lang.clone();
                tokio::task::spawn_blocking(move || {
                    ocr::recognize_cropped_region_sync(&f, target_rect, &ol)
                })
                .await
                .unwrap_or_else(|e| Err(anyhow::anyhow!("OCR join error: {}", e)))
                .unwrap_or_default()
            };

            if lines.is_empty() {
                let _ = tx.send(Err(anyhow::anyhow!("No text detected in this area")));
                return;
            }

            let trans_res = if use_ai {
                match client.translate_ocr_lines(&lines, &target_lang).await {
                    Ok(translated) => Ok(translated),
                    Err(_) => {
                        crate::services::translator::translate_ocr_lines_free(&lines, &target_lang)
                            .await
                    }
                }
            } else {
                crate::services::translator::translate_ocr_lines_free(&lines, &target_lang).await
            };

            let _ = tx.send(trans_res.map(|t| (lines, t)));
        });

        cx.spawn(async move |this, cx| {
            if let Some(res) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.state.is_loading = false;
                    match res {
                        Ok((recognized_lines, translated)) => {
                            if this.state.ocr_lines.is_empty() {
                                this.state.ocr_lines = recognized_lines;
                            }
                            for item in translated {
                                if let Some(existing) =
                                    this.state.translations.iter_mut().find(|t| t.id == item.id)
                                {
                                    existing.text = item.text;
                                } else {
                                    this.state.translations.push(item);
                                }
                            }
                            this.state.status_message = None;
                        }
                        Err(e) => {
                            this.state.status_message = Some(format!("{}", e));
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Launches Google Lens visual search in the background and immediately closes the region select window.
    pub fn trigger_google_lens_and_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rect = self.effective_selection();
        let jpeg_bytes = match self.frame.crop_to_jpeg_bytes(rect) {
            Ok(b) => b,
            Err(e) => {
                self.state.status_message = Some(format!("Encode error: {}", e));
                cx.notify();
                return;
            }
        };

        let w = rect.width;
        let h = rect.height;

        // Spawn on the persistent background Tokio runtime so closing the overlay window
        // does not cancel the upload or browser launch.
        tokio_runtime().spawn(async move {
            if let Err(err) = search_google_lens(jpeg_bytes, w, h).await {
                eprintln!("Google Lens error: {:#}", err);
            }
        });

        // Immediately close all overlay windows on the same click
        crate::ui::dismiss_overlay(window, cx);
    }

    pub fn trigger_ask_deepseek(
        &mut self,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !AppConfig::has_deepseek_api_key() {
            self.show_api_key_modal = true;
            cx.notify();
            return;
        }

        let rect = self.effective_selection();
        let webp_data_url = match self.frame.crop_to_vision_data_url(rect, 1280) {
            Ok(url) => url,
            Err(e) => {
                self.state.status_message = Some(format!("Encode error: {}", e));
                cx.notify();
                return;
            }
        };

        let local_ocr = ocr::extract_text_in_rect(&self.state.ocr_lines, &rect);

        self.state.mode = OverlayMode::AskDeepSeek;
        self.state.ai_prompt = prompt.clone();
        self.state.ai_markdown_response.clear();
        self.state.is_loading = true;
        self.state.status_message = None;

        self.prompt_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();

        let client = DeepSeekClient::new(self.config.clone());
        let (tx, mut rx) = mpsc::unbounded_channel::<StreamUpdate>();

        tokio_runtime().spawn(async move {
            let ocr_opt = if local_ocr.trim().is_empty() {
                None
            } else {
                Some(local_ocr.as_str())
            };
            let res = client
                .ask_vision_stream(&webp_data_url, ocr_opt, &prompt, |delta| {
                    let _ = tx.send(StreamUpdate::Delta(delta.to_string()));
                })
                .await;

            match res {
                Ok(()) => {
                    let _ = tx.send(StreamUpdate::Done);
                }
                Err(e) => {
                    let _ = tx.send(StreamUpdate::Error(format!("{:#}", e)));
                }
            }
        });

        cx.spawn(async move |this, cx| {
            while let Some(update) = rx.recv().await {
                let keep_going = this
                    .update(cx, |this, cx| match update {
                        StreamUpdate::Delta(delta) => {
                            this.state.ai_markdown_response.push_str(&delta);
                            cx.notify();
                            true
                        }
                        StreamUpdate::Done => {
                            this.state.is_loading = false;
                            cx.notify();
                            false
                        }
                        StreamUpdate::Error(err) => {
                            this.state.is_loading = false;
                            this.state.ai_markdown_response = format!("**Error:** `{}`", err);
                            cx.notify();
                            false
                        }
                    })
                    .unwrap_or(false);

                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn save_api_key_from_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.api_key_input.read(cx).value().trim().to_string();
        match AppConfig::set_deepseek_api_key(&key) {
            Ok(()) => {
                self.show_api_key_modal = false;
                self.api_key_input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.state.status_message = Some("API key saved".to_string());
            }
            Err(e) => {
                self.state.status_message = Some(format!("Keyring error: {}", e));
            }
        }
        cx.notify();
    }

    fn on_dismiss(&mut self, _: &DismissOverlay, window: &mut Window, cx: &mut Context<Self>) {
        crate::ui::dismiss_overlay(window, cx);
    }

    fn on_copy_action(&mut self, _: &CopySelection, window: &mut Window, cx: &mut Context<Self>) {
        self.copy_selected_image(cx);
        crate::ui::dismiss_overlay(window, cx);
    }

    fn on_save_png_action(
        &mut self,
        _: &SaveSelectionPng,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_selected_screenshot(cx);
    }

    fn render_top_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let hint = self
            .state
            .status_message
            .clone()
            .unwrap_or_else(|| "Drag to select region  •  Esc to close".to_string());

        let reload_cb = self.on_reload_hotkey.clone();

        div()
            .id("top-status-bar")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .absolute()
            .top(px(12.0))
            .left(px(0.0))
            .right(px(0.0))
            .flex()
            .justify_center()
            .child(
                h_flex()
                    .gap_2()
                    .px_2p5()
                    .py_1()
                    .rounded(px(3.0))
                    .bg(rgb(BG_WINDOW))
                    .border_1()
                    .border_color(rgb(BORDER_LINE))
                    .items_center()
                    .child(div().text_xs().text_color(rgb(TEXT_SUB)).child(hint))
                    .child(icon_btn(
                        "open-prefs-btn",
                        IconName::Settings,
                        cx.listener(move |_, _, window, cx| {
                            let reload_cb = reload_cb.clone();
                            window.remove_window();
                            cx.defer(move |cx| {
                                crate::ui::close_all_overlay_windows(cx);
                                let reload_for_capture = reload_cb.clone();
                                let reload_for_settings = reload_cb.clone();
                                let _ = open_settings_window(
                                    cx,
                                    move |cx| {
                                        if let Ok(frame) = crate::capture::capture_primary_screen()
                                        {
                                            let cfg = AppConfig::load();
                                            let r = reload_for_capture.clone();
                                            let _ = open_overlay_window(cx, frame, cfg, move |hk| {
                                                r(hk)
                                            });
                                        }
                                    },
                                    move |hk| reload_for_settings(hk),
                                );
                            });
                        }),
                    )),
            )
    }

    fn render_api_key_modal(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("api-key-modal")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .absolute()
            .inset_0()
            .bg(rgba(0x00000077))
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w(px(380.0))
                    .p_3p5()
                    .gap_2p5()
                    .rounded(px(4.0))
                    .bg(rgb(BG_SURFACE))
                    .border_1()
                    .border_color(rgb(BORDER_FOCUS))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT_MAIN))
                            .child("DeepSeek API Key Required"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_SUB))
                            .child("Stored in Windows Credential Manager."),
                    )
                    .child(Input::new(&self.api_key_input))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_1p5()
                            .child(tool_btn(
                                "cancel-api-key",
                                None,
                                "Cancel",
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.show_api_key_modal = false;
                                    cx.notify();
                                }),
                            ))
                            .child(tool_btn(
                                "save-api-key",
                                Some(IconName::Check),
                                "Save",
                                true,
                                cx.listener(|this, _, window, cx| {
                                    this.save_api_key_from_input(window, cx);
                                }),
                            )),
                    ),
            )
    }
}

impl Render for OverlayView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lw = self.frame.logical_width();
        let lh = self.frame.logical_height();
        let scale = self.frame.scale_factor;
        let selection = self.state.selection;
        let is_translate_mode = self.state.mode == OverlayMode::InPlaceTranslate;

        // Render subtle clickable OCR boxes or in-place translations strictly inside the selected region
        let region_ocr_elements: Vec<_> = if let Some(sel) = selection.filter(|_| !self.is_dragging)
        {
            self.state
                .ocr_lines
                .iter()
                .filter(|line| line.bounds.intersects(&sel))
                .map(|line| {
                    let bx = line.bounds.x / scale;
                    let by = line.bounds.y / scale;
                    let bw = (line.bounds.width / scale).max(16.0);
                    let bh = (line.bounds.height / scale).max(14.0);
                    let line_id = line.id;
                    let line_text = line.text.clone();

                    let translated = if is_translate_mode {
                        self.state
                            .translations
                            .iter()
                            .find(|t| t.id == line_id)
                            .map(|t| t.text.clone())
                    } else {
                        None
                    };

                    div()
                        .id(("ocr-line", line_id))
                        .absolute()
                        .left(px(bx))
                        .top(px(by))
                        .min_w(px(bw))
                        .min_h(px(bh))
                        .cursor(CursorStyle::PointingHand)
                        .when_some(translated, |el, trans| {
                            el.px_1p5()
                                .py_0p5()
                                .rounded(px(2.0))
                                .bg(rgb(BG_WINDOW))
                                .border_1()
                                .border_color(rgb(BORDER_FOCUS))
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(TEXT_MAIN))
                                        .child(SharedString::from(trans)),
                                )
                        })
                        .when(!is_translate_mode, |el| {
                            el.rounded(px(2.0))
                                .bg(rgba(0xffffff12))
                                .border_1()
                                .border_color(rgba(0xffffff38))
                                .hover(|h| h.bg(rgba(0xffffff28)).border_color(rgba(0xffffff88)))
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if copy_text_to_clipboard(&line_text).is_ok() {
                                    this.state.status_message =
                                        Some(format!("Copied: \"{}\"", line_text));
                                    cx.notify();
                                }
                            }),
                        )
                })
                .collect()
        } else {
            Vec::new()
        };

        div()
            .id("winlook-overlay-root")
            .key_context("WinLookOverlay")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_dismiss))
            .on_action(cx.listener(Self::on_copy_action))
            .on_action(cx.listener(Self::on_save_png_action))
            .relative()
            .w(px(lw))
            .h(px(lh))
            .overflow_hidden()
            .cursor(CursorStyle::Crosshair)
            // 1. Frozen screen image base layer
            .child(
                img(self.frame.render_image.clone())
                    .w(px(lw))
                    .h(px(lh))
                    .object_fit(ObjectFit::Fill),
            )
            // 2. Interactive drag canvas & neutral scrim
            .child(
                div()
                    .id("drag-canvas")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            let x = f32::from(event.position.x);
                            let y = f32::from(event.position.y);
                            this.drag_start = Some((x, y));
                            this.is_dragging = true;
                            this.state.selection = None;
                            this.state.ocr_lines.clear();
                            this.state.auto_ocr_text.clear();
                            this.state.status_message = None;
                            this.state.mode = OverlayMode::RegionSelect;
                            cx.notify();

                            let current_id = cx.entity_id();
                            cx.defer(move |cx| {
                                for win in cx.windows() {
                                    if let Some(view) = win.downcast::<OverlayView>() {
                                        if view.entity(cx).ok().map(|e| e.entity_id())
                                            != Some(current_id)
                                        {
                                            let _ = view.update(cx, |other, _window, cx| {
                                                if other.state.selection.is_some() {
                                                    other.state.selection = None;
                                                    other.state.ocr_lines.clear();
                                                    other.state.auto_ocr_text.clear();
                                                    other.state.translations.clear();
                                                    other.state.status_message = None;
                                                    other.state.mode = OverlayMode::RegionSelect;
                                                    cx.notify();
                                                }
                                            });
                                        }
                                    }
                                }
                            });
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        if !this.is_dragging {
                            return;
                        }
                        if let Some((start_x, start_y)) = this.drag_start {
                            let cur_x = f32::from(event.position.x);
                            let cur_y = f32::from(event.position.y);
                            let rect = ScreenRect::from_logical_points(
                                start_x,
                                start_y,
                                cur_x,
                                cur_y,
                                this.frame.scale_factor,
                                this.frame.width,
                                this.frame.height,
                            );
                            this.state.selection = Some(rect);
                            cx.notify();
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseUpEvent, _, cx| {
                            if !this.is_dragging {
                                return;
                            }
                            this.is_dragging = false;
                            if let Some((start_x, start_y)) = this.drag_start.take() {
                                let end_x = f32::from(event.position.x);
                                let end_y = f32::from(event.position.y);
                                let rect = ScreenRect::from_logical_points(
                                    start_x,
                                    start_y,
                                    end_x,
                                    end_y,
                                    this.frame.scale_factor,
                                    this.frame.width,
                                    this.frame.height,
                                );
                                if rect.is_meaningful() {
                                    this.state.selection = Some(rect);
                                    // Fast cropped-region multi-language Auto-OCR on selection
                                    this.run_auto_ocr_for_selection(cx);
                                } else {
                                    this.state.selection = None;
                                }
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|_, _, window, cx| {
                            crate::ui::dismiss_overlay(window, cx);
                        }),
                    )
                    .when(selection.is_none(), |canvas| {
                        canvas.bg(rgba(0x00000048))
                    })
                    .when_some(selection, |canvas, sel| {
                        let (sx, sy, sw, sh) = sel.to_logical(scale);
                        let sel_left = sx.clamp(0.0, lw);
                        let sel_top = sy.clamp(0.0, lh);
                        let sel_right = (sx + sw).clamp(sel_left, lw);
                        let sel_bottom = (sy + sh).clamp(sel_top, lh);
                        let sel_w = sel_right - sel_left;
                        let sel_h = sel_bottom - sel_top;

                        let tag_x = sel_left.clamp(4.0, (lw - 90.0).max(4.0));
                        let tag_y = if sel_top >= 24.0 {
                            sel_top - 22.0
                        } else if sel_bottom + 24.0 <= lh {
                            sel_top + 4.0
                        } else {
                            (lh - 24.0).max(4.0)
                        };

                        canvas
                            // 1. Top scrim rectangle
                            .child(
                                div()
                                    .absolute()
                                    .left(px(0.0))
                                    .top(px(0.0))
                                    .w(px(lw))
                                    .h(px(sel_top))
                                    .bg(rgba(0x00000078)),
                            )
                            // 2. Bottom scrim rectangle
                            .child(
                                div()
                                    .absolute()
                                    .left(px(0.0))
                                    .top(px(sel_bottom))
                                    .w(px(lw))
                                    .h(px(lh - sel_bottom))
                                    .bg(rgba(0x00000078)),
                            )
                            // 3. Left scrim rectangle
                            .child(
                                div()
                                    .absolute()
                                    .left(px(0.0))
                                    .top(px(sel_top))
                                    .w(px(sel_left))
                                    .h(px(sel_h))
                                    .bg(rgba(0x00000078)),
                            )
                            // 4. Right scrim rectangle
                            .child(
                                div()
                                    .absolute()
                                    .left(px(sel_right))
                                    .top(px(sel_top))
                                    .w(px(lw - sel_right))
                                    .h(px(sel_h))
                                    .bg(rgba(0x00000078)),
                            )
                            // Neutral 1px selection outline
                            .child(
                                div()
                                    .absolute()
                                    .left(px(sel_left))
                                    .top(px(sel_top))
                                    .w(px(sel_w))
                                    .h(px(sel_h))
                                    .border_1()
                                    .border_color(rgb(SELECTION_STROKE)),
                            )
                            // Flat utilitarian dimension label
                            .child(
                                div()
                                    .absolute()
                                    .left(px(tag_x))
                                    .top(px(tag_y))
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(2.0))
                                    .bg(rgb(BG_WINDOW))
                                    .border_1()
                                    .border_color(rgb(BORDER_LINE))
                                    .text_xs()
                                    .text_color(rgb(TEXT_SUB))
                                    .child(format!("{} × {}", sel.width, sel.height)),
                            )
                    }),
            )
            // 3. Auto-OCR lines & In-Place Translations inside selection
            .children(region_ocr_elements)
            // 4. Top status bar (only when no region selected or status toast active)
            .when(
                selection.is_none() || self.state.status_message.is_some(),
                |root| root.child(self.render_top_status_bar(cx)),
            )
            // 5. Selection Toolbar + Auto-OCR Box + AI Answer Box
            .when(selection.is_some() && !self.is_dragging, |root| {
                root.child(render_floating_inspector(self, cx))
            })
            // 6. API Key modal if needed
            .when(self.show_api_key_modal, |root| {
                root.child(self.render_api_key_modal(cx))
            })
    }
}
