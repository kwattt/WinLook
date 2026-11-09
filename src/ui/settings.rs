use crate::capture;
use crate::config::{
    AppConfig, DEEPSEEK_MODELS, SUPPORTED_LANGUAGES, format_recorded_hotkey,
};
use crate::ocr;
use crate::ui::overlay::tokio_runtime;
use crate::ui::{
    BG_ACTIVE, BG_CONTROL, BG_HOVER, BG_SURFACE, BG_WINDOW, BORDER_FOCUS, BORDER_LINE, TEXT_DIM,
    TEXT_MAIN, TEXT_SUB, icon_btn, tool_btn,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement as _,
    Styled, Subscription, Window, div, px, rgb,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::switch::Switch;
use gpui_component::{IconName, IndexPath, Sizable as _, h_flex, v_flex};
use std::rc::Rc;
use tokio::sync::mpsc;

const AUTO_OCR_LABEL: &str = "Auto (Multi / Japanese / CJK)";
const ENGINE_GOOGLE_LABEL: &str = "Google Translate (Default, No AI)";
const ENGINE_DEEPSEEK_LABEL: &str = "DeepSeek AI";
const REASONING_OFF_LABEL: &str = "Disabled (Fast Direct Answer)";
const REASONING_ON_LABEL: &str = "Enabled (DeepSeek Thinking)";

pub struct SettingsWindowView {
    config: AppConfig,
    autostart_enabled: bool,
    recorder_focus: FocusHandle,
    is_recording_hotkey: bool,
    ocr_lang_select: Entity<SelectState<Vec<SharedString>>>,
    target_lang_select: Entity<SelectState<Vec<SharedString>>>,
    trans_engine_select: Entity<SelectState<Vec<SharedString>>>,
    model_select: Entity<SelectState<Vec<SharedString>>>,
    reasoning_select: Entity<SelectState<Vec<SharedString>>>,
    folder_input: Entity<InputState>,
    api_key_input: Entity<InputState>,
    status_text: String,
    on_trigger_capture: Rc<dyn Fn(&mut App)>,
    on_reload_hotkey: Rc<dyn Fn(String)>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for SettingsWindowView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.recorder_focus.clone()
    }
}

impl SettingsWindowView {
    pub fn new(
        on_trigger_capture: impl Fn(&mut App) + 'static,
        on_reload_hotkey: impl Fn(String) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let config = AppConfig::load();
        let autostart_enabled = AppConfig::is_autostart_enabled();
        let recorder_focus = cx.focus_handle();

        // 1. OCR Language Dropdown Select
        let raw_ocr_langs = ocr::available_ocr_languages();
        let ocr_items: Vec<SharedString> = raw_ocr_langs
            .iter()
            .map(|l| {
                if l.eq_ignore_ascii_case("auto") {
                    SharedString::from(AUTO_OCR_LABEL)
                } else {
                    SharedString::from(l.clone())
                }
            })
            .collect();
        let ocr_selected_row = raw_ocr_langs
            .iter()
            .position(|l| l.eq_ignore_ascii_case(&config.ocr_language))
            .unwrap_or(0);
        let ocr_lang_select = cx.new(|cx| {
            SelectState::new(
                ocr_items,
                Some(IndexPath::default().row(ocr_selected_row)),
                window,
                cx,
            )
        });

        // 2. Target Translation Language Dropdown Select
        let target_items: Vec<SharedString> = SUPPORTED_LANGUAGES
            .iter()
            .map(|&s| SharedString::from(s))
            .collect();
        let target_selected_row = SUPPORTED_LANGUAGES
            .iter()
            .position(|&l| l.eq_ignore_ascii_case(&config.target_language))
            .unwrap_or(0);
        let target_lang_select = cx.new(|cx| {
            SelectState::new(
                target_items,
                Some(IndexPath::default().row(target_selected_row)),
                window,
                cx,
            )
        });

        // 3. Translation Engine Dropdown Select (Google non-AI is index 0 / default)
        let engine_items: Vec<SharedString> = vec![
            SharedString::from(ENGINE_GOOGLE_LABEL),
            SharedString::from(ENGINE_DEEPSEEK_LABEL),
        ];
        let engine_row = if config.use_ai_translation && AppConfig::has_deepseek_api_key() {
            1
        } else {
            0
        };
        let trans_engine_select = cx.new(|cx| {
            SelectState::new(
                engine_items,
                Some(IndexPath::default().row(engine_row)),
                window,
                cx,
            )
        });

        // 4. DeepSeek Model Dropdown Select
        let model_items: Vec<SharedString> = DEEPSEEK_MODELS
            .iter()
            .map(|&m| SharedString::from(m))
            .collect();
        let model_row = DEEPSEEK_MODELS
            .iter()
            .position(|&m| m.eq_ignore_ascii_case(&config.deepseek_model))
            .unwrap_or(0);
        let model_select = cx.new(|cx| {
            SelectState::new(
                model_items,
                Some(IndexPath::default().row(model_row)),
                window,
                cx,
            )
        });

        // 5. AI Reasoning (Thinking Mode) Dropdown Select (Disabled by default)
        let reasoning_items: Vec<SharedString> = vec![
            SharedString::from(REASONING_OFF_LABEL),
            SharedString::from(REASONING_ON_LABEL),
        ];
        let reasoning_row = if config.enable_ai_reasoning { 1 } else { 0 };
        let reasoning_select = cx.new(|cx| {
            SelectState::new(
                reasoning_items,
                Some(IndexPath::default().row(reasoning_row)),
                window,
                cx,
            )
        });

        // 6. Text Inputs
        let folder_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(config.screenshot_dir.clone())
                .placeholder("C:\\Users\\...\\Pictures\\WinLook")
        });

        let api_key_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Paste DeepSeek API key (sk-...) and press Enter")
        });

        // Subscriptions for Selects & Inputs
        let sub_ocr = cx.subscribe_in(
            &ocr_lang_select,
            window,
            |this: &mut Self, _, event: &SelectEvent<Vec<SharedString>>, _, cx| {
                if let SelectEvent::Confirm(Some(val)) = event {
                    let code = if val.as_ref() == AUTO_OCR_LABEL {
                        "auto".to_string()
                    } else {
                        val.to_string()
                    };
                    this.config.ocr_language = code.clone();
                    let _ = this.config.save();
                    this.status_text = format!("OCR language set to {}", val);
                    cx.notify();
                }
            },
        );

        let sub_target = cx.subscribe_in(
            &target_lang_select,
            window,
            |this: &mut Self, _, event: &SelectEvent<Vec<SharedString>>, _, cx| {
                if let SelectEvent::Confirm(Some(val)) = event {
                    this.config.target_language = val.to_string();
                    let _ = this.config.save();
                    this.status_text = format!("Target language set to {}", val);
                    cx.notify();
                }
            },
        );

        let sub_engine = cx.subscribe_in(
            &trans_engine_select,
            window,
            |this: &mut Self, _, event: &SelectEvent<Vec<SharedString>>, _, cx| {
                if let SelectEvent::Confirm(Some(val)) = event {
                    let use_ai = val.as_ref() == ENGINE_DEEPSEEK_LABEL;
                    this.config.use_ai_translation = use_ai;
                    let _ = this.config.save();
                    this.status_text = format!("Translation engine: {}", val);
                    cx.notify();
                }
            },
        );

        let sub_model = cx.subscribe_in(
            &model_select,
            window,
            |this: &mut Self, _, event: &SelectEvent<Vec<SharedString>>, _, cx| {
                if let SelectEvent::Confirm(Some(val)) = event {
                    this.config.deepseek_model = val.to_string();
                    let _ = this.config.save();
                    this.status_text = format!("DeepSeek model set to {}", val);
                    cx.notify();
                }
            },
        );

        let sub_reasoning = cx.subscribe_in(
            &reasoning_select,
            window,
            |this: &mut Self, _, event: &SelectEvent<Vec<SharedString>>, _, cx| {
                if let SelectEvent::Confirm(Some(val)) = event {
                    let enabled = val.as_ref() == REASONING_ON_LABEL;
                    this.config.enable_ai_reasoning = enabled;
                    let _ = this.config.save();
                    this.status_text = format!(
                        "AI Reasoning: {}",
                        if enabled { "Enabled" } else { "Disabled" }
                    );
                    cx.notify();
                }
            },
        );

        let sub_dir = cx.subscribe_in(
            &folder_input,
            window,
            |this: &mut Self, _, event: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.save_folder_from_input(cx);
                }
            },
        );

        let sub_key = cx.subscribe_in(
            &api_key_input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.save_api_key(window, cx);
                }
            },
        );

        let status_text = format!("Active shortcut: {}", config.hotkey);

        Self {
            config,
            autostart_enabled,
            recorder_focus,
            is_recording_hotkey: false,
            ocr_lang_select,
            target_lang_select,
            trans_engine_select,
            model_select,
            reasoning_select,
            folder_input,
            api_key_input,
            status_text,
            on_trigger_capture: Rc::new(on_trigger_capture),
            on_reload_hotkey: Rc::new(on_reload_hotkey),
            _subscriptions: vec![
                sub_ocr,
                sub_target,
                sub_engine,
                sub_model,
                sub_reasoning,
                sub_dir,
                sub_key,
            ],
        }
    }

    fn handle_recorder_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_recording_hotkey {
            return;
        }

        cx.stop_propagation();
        let ks = &event.keystroke;

        if ks.key.eq_ignore_ascii_case("escape")
            && !ks.modifiers.control
            && !ks.modifiers.alt
            && !ks.modifiers.shift
            && !ks.modifiers.platform
        {
            self.is_recording_hotkey = false;
            (self.on_reload_hotkey)(self.config.hotkey.clone());
            self.status_text = "Keybind recording cancelled".to_string();
            cx.notify();
            return;
        }

        if let Some(combo) = format_recorded_hotkey(
            ks.modifiers.control,
            ks.modifiers.alt,
            ks.modifiers.shift,
            ks.modifiers.platform,
            &ks.key,
        ) {
            self.is_recording_hotkey = false;
            self.config.hotkey = combo.clone();
            let _ = self.config.save();
            (self.on_reload_hotkey)(combo.clone());
            self.status_text = format!("Shortcut updated to {}", combo);
            cx.notify();
        }
    }

    fn set_autostart(&mut self, enabled: bool, cx: &mut Context<Self>) {
        match AppConfig::set_autostart_enabled(enabled) {
            Ok(()) => {
                self.autostart_enabled = enabled;
                self.status_text = if enabled {
                    "Enabled: WinLook will start automatically with Windows".to_string()
                } else {
                    "Disabled Windows startup".to_string()
                };
            }
            Err(e) => {
                self.status_text = format!("Startup error: {}", e);
            }
        }
        cx.notify();
    }

    fn save_folder_from_input(&mut self, cx: &mut Context<Self>) {
        let dir = self.folder_input.read(cx).value().trim().to_string();
        if dir.is_empty() {
            return;
        }
        self.config.screenshot_dir = dir;
        match self.config.ensure_screenshots_dir() {
            Ok(resolved) => {
                let _ = self.config.save();
                self.status_text = format!("Save folder set to {}", resolved.display());
            }
            Err(e) => {
                self.status_text = format!("Invalid folder: {}", e);
            }
        }
        cx.notify();
    }

    fn browse_for_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let initial = self.config.screenshot_dir.clone();
        let (tx, mut rx) = mpsc::unbounded_channel::<Option<String>>();

        tokio_runtime().spawn_blocking(move || {
            let picked = AppConfig::pick_folder_dialog(&initial);
            let _ = tx.send(picked);
        });

        let folder_input = self.folder_input.clone();
        cx.spawn_in(window, async move |this, cx| {
            if let Some(Some(chosen)) = rx.recv().await {
                let _ = this.update_in(cx, |this, window, cx| {
                    folder_input.update(cx, |input, cx| input.set_value(&chosen, window, cx));
                    this.config.screenshot_dir = chosen.clone();
                    let _ = this.config.ensure_screenshots_dir();
                    let _ = this.config.save();
                    this.status_text = format!("Save folder: {}", chosen);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn save_api_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.api_key_input.read(cx).value().trim().to_string();
        if key.is_empty() {
            self.status_text = "API key cannot be empty".to_string();
            cx.notify();
            return;
        }

        match AppConfig::set_deepseek_api_key(&key) {
            Ok(()) => {
                self.api_key_input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.status_text =
                    "DeepSeek API key saved in Windows Credential Manager".to_string();
            }
            Err(e) => {
                self.status_text = format!("Error: {}", e);
            }
        }
        cx.notify();
    }
}

impl Render for SettingsWindowView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let masked_key = AppConfig::masked_api_key();
        let has_key = masked_key.is_some();
        let active_hotkey = self.config.hotkey.clone();
        let auto_copy_save = self.config.auto_copy_on_save;
        let auto_copy_ocr = self.config.auto_copy_ocr;
        let autostart = self.autostart_enabled;
        let is_recording = self.is_recording_hotkey;
        let status_text = self.status_text.clone();

        let recorder_label: SharedString = if is_recording {
            "Listening... Press key combination (Esc to cancel)".into()
        } else {
            format!("{}   (Click to record new keybind)", active_hotkey).into()
        };

        div()
            .id("winlook-preferences-root")
            .track_focus(&self.recorder_focus)
            .on_key_down(cx.listener(Self::handle_recorder_key_down))
            .size_full()
            .bg(rgb(BG_WINDOW))
            .text_color(rgb(TEXT_MAIN))
            .flex()
            .flex_col()
            // ================================================================
            // 1. TOP ACTION BAR (FIXED)
            // ================================================================
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .px_4()
                    .py_2p5()
                    .bg(rgb(BG_SURFACE))
                    .border_b_1()
                    .border_color(rgb(BORDER_LINE))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(tool_btn(
                                "act-capture-region",
                                Some(IconName::Frame),
                                format!("Select Region ({})", active_hotkey),
                                true,
                                cx.listener(|this, _, window, cx| {
                                    let cb = this.on_trigger_capture.clone();
                                    window.remove_window();
                                    cx.defer(move |cx| {
                                        cb(cx);
                                    });
                                }),
                            ))
                            .child(tool_btn(
                                "act-fullscreen-shot",
                                Some(IconName::Maximize),
                                "Fullscreen Screenshot",
                                false,
                                cx.listener(|this, _, _, cx| {
                                    match capture::capture_primary_screen() {
                                        Ok(frame) => {
                                            let rect = frame.full_rect();
                                            match frame.save_crop_to_disk(rect, &this.config) {
                                                Ok(path) => {
                                                    let name = path
                                                        .file_name()
                                                        .and_then(|n| n.to_str())
                                                        .unwrap_or("screenshot.png");
                                                    this.status_text = format!("Saved {}", name);
                                                }
                                                Err(e) => {
                                                    this.status_text = format!("Error: {}", e);
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            this.status_text = format!("Capture error: {}", e);
                                        }
                                    }
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(icon_btn(
                        "act-open-folder",
                        IconName::FolderOpen,
                        cx.listener(|this, _, _, cx| {
                            if let Ok(dir) = this.config.ensure_screenshots_dir() {
                                let _ = open::that(&dir);
                                this.status_text = format!("Opened {}", dir.display());
                                cx.notify();
                            }
                        }),
                    )),
            )
            // ================================================================
            // 2. SCROLLABLE PREFERENCES BODY (NO OVERFLOW)
            // ================================================================
            .child(
                div()
                    .id("settings-scroll-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(
                        v_flex()
                            .px_4()
                            .py_2()
                            // ------------------------------------------------
                            // SECTION 1: CAPTURE SHORTCUT (CLICK TO RECORD)
                            // ------------------------------------------------
                            .child(
                                v_flex()
                                    .py_2p5()
                                    .gap_1p5()
                                    .border_b_1()
                                    .border_color(rgb(BORDER_LINE))
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(rgb(TEXT_MAIN))
                                                    .child("Capture Shortcut"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(TEXT_DIM))
                                                    .child("Click button and press keys (e.g. Alt+Space, Ctrl+Shift+S, F9)"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("keybind-recorder-btn")
                                            .w_full()
                                            .cursor_pointer()
                                            .px_3()
                                            .py_1p5()
                                            .rounded(px(3.0))
                                            .bg(rgb(if is_recording {
                                                BG_ACTIVE
                                            } else {
                                                BG_CONTROL
                                            }))
                                            .border_1()
                                            .border_color(rgb(if is_recording {
                                                BORDER_FOCUS
                                            } else {
                                                BORDER_LINE
                                            }))
                                            .hover(|h| {
                                                h.bg(rgb(BG_HOVER)).border_color(rgb(BORDER_FOCUS))
                                            })
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(TEXT_MAIN))
                                            .child(recorder_label)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.is_recording_hotkey =
                                                    !this.is_recording_hotkey;
                                                if this.is_recording_hotkey {
                                                    (this.on_reload_hotkey)(String::new());
                                                    this.recorder_focus.focus(window, cx);
                                                    this.status_text =
                                                        "Press desired key combination now..."
                                                            .to_string();
                                                } else {
                                                    (this.on_reload_hotkey)(
                                                        this.config.hotkey.clone(),
                                                    );
                                                    this.status_text = format!(
                                                        "Active shortcut: {}",
                                                        this.config.hotkey
                                                    );
                                                }
                                                cx.notify();
                                            })),
                                    ),
                            )
                            // ------------------------------------------------
                            // SECTION 2: GENERAL & CLIPBOARD SWITCHES
                            // ------------------------------------------------
                            .child(
                                v_flex()
                                    .py_2p5()
                                    .gap_2()
                                    .border_b_1()
                                    .border_color(rgb(BORDER_LINE))
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(TEXT_MAIN))
                                                    .child("Start WinLook automatically when Windows starts"),
                                            )
                                            .child(
                                                Switch::new("sw-autostart")
                                                    .small()
                                                    .checked(autostart)
                                                    .on_click(cx.listener(
                                                        |this, checked, _, cx| {
                                                            this.set_autostart(*checked, cx);
                                                        },
                                                    )),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(TEXT_MAIN))
                                                    .child("Auto-copy OCR text to clipboard on region select"),
                                            )
                                            .child(
                                                Switch::new("sw-auto-copy-ocr")
                                                    .small()
                                                    .checked(auto_copy_ocr)
                                                    .on_click(cx.listener(
                                                        |this, checked, _, cx| {
                                                            this.config.auto_copy_ocr = *checked;
                                                            let _ = this.config.save();
                                                            cx.notify();
                                                        },
                                                    )),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(TEXT_MAIN))
                                                    .child("Auto-copy screenshot image to clipboard when saving"),
                                            )
                                            .child(
                                                Switch::new("sw-auto-copy-save")
                                                    .small()
                                                    .checked(auto_copy_save)
                                                    .on_click(cx.listener(
                                                        |this, checked, _, cx| {
                                                            this.config.auto_copy_on_save =
                                                                *checked;
                                                            let _ = this.config.save();
                                                            cx.notify();
                                                        },
                                                    )),
                                            ),
                                    ),
                            )
                            // ------------------------------------------------
                            // SECTION 3: SCREENSHOT SAVE FOLDER
                            // ------------------------------------------------
                            .child(
                                v_flex()
                                    .py_2p5()
                                    .gap_1p5()
                                    .border_b_1()
                                    .border_color(rgb(BORDER_LINE))
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(TEXT_MAIN))
                                            .child("Screenshot Save Folder"),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1p5()
                                            .items_center()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .child(Input::new(&self.folder_input)),
                                            )
                                            .child(tool_btn(
                                                "btn-browse-folder",
                                                Some(IconName::Folder),
                                                "Browse...",
                                                false,
                                                cx.listener(|this, _, window, cx| {
                                                    this.browse_for_folder(window, cx);
                                                }),
                                            ))
                                            .child(tool_btn(
                                                "btn-save-folder",
                                                Some(IconName::Check),
                                                "Set",
                                                false,
                                                cx.listener(|this, _, _, cx| {
                                                    this.save_folder_from_input(cx);
                                                }),
                                            )),
                                    ),
                            )
                            // ------------------------------------------------
                            // SECTION 4: OCR, TRANSLATION & AI DROPDOWN SELECTS
                            // ------------------------------------------------
                            .child(
                                v_flex()
                                    .py_2p5()
                                    .gap_2()
                                    .border_b_1()
                                    .border_color(rgb(BORDER_LINE))
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .items_center()
                                            .gap_3()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(rgb(TEXT_MAIN))
                                                    .child("OCR Recognition Language"),
                                            )
                                            .child(
                                                div().w(px(250.0)).child(
                                                    Select::new(&self.ocr_lang_select).small(),
                                                ),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .items_center()
                                            .gap_3()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(rgb(TEXT_MAIN))
                                                    .child("Translate Target Language"),
                                            )
                                            .child(
                                                div().w(px(250.0)).child(
                                                    Select::new(&self.target_lang_select).small(),
                                                ),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .items_center()
                                            .gap_3()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(rgb(TEXT_MAIN))
                                                    .child("Translation Engine"),
                                            )
                                            .child(
                                                div().w(px(250.0)).child(
                                                    Select::new(&self.trans_engine_select)
                                                        .small()
                                                        .disabled(!has_key),
                                                ),
                                            ),
                                    )
                                    .when(has_key, |sec| {
                                        sec.child(
                                            h_flex()
                                                .justify_between()
                                                .items_center()
                                                .gap_3()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(rgb(TEXT_MAIN))
                                                        .child("DeepSeek AI Model"),
                                                )
                                                .child(
                                                    div()
                                                        .w(px(250.0))
                                                        .child(Select::new(&self.model_select).small()),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .justify_between()
                                                .items_center()
                                                .gap_3()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(rgb(TEXT_MAIN))
                                                        .child("AI Reasoning (Thinking Mode)"),
                                                )
                                                .child(
                                                    div().w(px(250.0)).child(
                                                        Select::new(&self.reasoning_select).small(),
                                                    ),
                                                ),
                                        )
                                    }),
                            )
                            // ------------------------------------------------
                            // SECTION 5: DEEPSEEK API KEY
                            // ------------------------------------------------
                            .child(
                                v_flex()
                                    .py_2p5()
                                    .gap_1p5()
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(rgb(TEXT_MAIN))
                                                    .child("DeepSeek API Key (Optional)"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(TEXT_SUB))
                                                    .child(match masked_key {
                                                        Some(mask) => format!("Saved ({})", mask),
                                                        None => {
                                                            "Not configured (using free Google Translate)"
                                                                .to_string()
                                                        }
                                                    }),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1p5()
                                            .items_center()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .child(Input::new(&self.api_key_input)),
                                            )
                                            .child(tool_btn(
                                                "btn-save-api-key",
                                                Some(IconName::Check),
                                                "Save Key",
                                                true,
                                                cx.listener(|this, _, window, cx| {
                                                    this.save_api_key(window, cx);
                                                }),
                                            )),
                                    ),
                            ),
                    ),
            )
            // ================================================================
            // 3. BOTTOM STATUS BAR (FIXED)
            // ================================================================
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .px_4()
                    .py_2()
                    .bg(rgb(BG_SURFACE))
                    .border_t_1()
                    .border_color(rgb(BORDER_LINE))
                    .child(div().text_xs().text_color(rgb(TEXT_SUB)).child(status_text))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_DIM))
                            .child("WinLook"),
                    ),
            )
    }
}
