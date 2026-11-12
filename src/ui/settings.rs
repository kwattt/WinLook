use crate::capture;
use crate::config::{
    AppConfig, DEEPSEEK_MODELS, SUPPORTED_LANGUAGES, format_recorded_hotkey,
};
use crate::ocr;
use crate::ui::overlay::tokio_runtime;
use crate::ui::{
    BG_ACTIVE, BG_CONTROL, BG_HOVER, BG_INPUT, BG_SURFACE, BG_WINDOW, BORDER_FOCUS, BORDER_LINE,
    TEXT_DIM, TEXT_MAIN, TEXT_SUB, icon_btn, tool_btn,
};
use crate::updater::{
    DownloadProgress, GitHubRelease, GitHubUpdaterClient, ReleaseAsset, UpdateCheckResult,
    UpdateStatus,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement as _,
    Styled, Subscription, Window, div, px, rgb,
};
use gpui_component::Icon;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::spinner::Spinner;
use gpui_component::switch::Switch;
use gpui_component::{IconName, IndexPath, Sizable as _, h_flex, v_flex};
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

static PENDING_UPDATE_CHECK: AtomicBool = AtomicBool::new(false);
static PENDING_UPDATE_RESULT: Mutex<Option<UpdateCheckResult>> = Mutex::new(None);

/// Requests that the next opened Settings window immediately initiates an update check.
pub fn request_update_check_on_next_open() {
    PENDING_UPDATE_CHECK.store(true, Ordering::SeqCst);
}

/// Stores a pre-fetched update check result (e.g. from background startup check)
/// to be displayed when the Settings window opens.
pub fn set_pending_update_result(result: UpdateCheckResult) {
    if let Ok(mut lock) = PENDING_UPDATE_RESULT.lock() {
        *lock = Some(result);
    }
}

/// State held by SettingsWindowView for the Application Updates section.
#[derive(Debug, Clone)]
pub enum UpdaterUiState {
    /// Idle state: no check active, ready to check.
    Idle,
    /// Currently querying GitHub Releases API.
    Checking,
    /// Verified up-to-date.
    UpToDate { version: String },
    /// Newer GitHub release is available.
    Available {
        release: Box<GitHubRelease>,
        target_asset: Option<ReleaseAsset>,
    },
    /// Actively downloading the installer binary.
    Downloading {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        percent: f32,
    },
    /// Download complete, launching detached installer process.
    Installing,
    /// Error encountered during check or download.
    Error(String),
}

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
    updater_ui_state: UpdaterUiState,
    notes_expanded: bool,
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

        let mut auto_trigger_check = false;
        let initial_updater_state = if let Ok(mut lock) = PENDING_UPDATE_RESULT.lock() {
            if let Some(res) = lock.take() {
                match res.status {
                    UpdateStatus::Available(release) => UpdaterUiState::Available {
                        release: Box::new(release),
                        target_asset: res.target_asset,
                    },
                    UpdateStatus::UpToDate => UpdaterUiState::UpToDate {
                        version: res.current_version,
                    },
                    UpdateStatus::RateLimited(reset) => {
                        UpdaterUiState::Error(format_rate_limit_msg(reset))
                    }
                    UpdateStatus::Failed(err) => UpdaterUiState::Error(err),
                }
            } else if PENDING_UPDATE_CHECK.swap(false, Ordering::SeqCst) {
                auto_trigger_check = true;
                UpdaterUiState::Checking
            } else {
                UpdaterUiState::Idle
            }
        } else {
            UpdaterUiState::Idle
        };

        let view = Self {
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
            updater_ui_state: initial_updater_state,
            notes_expanded: false,
        };

        if auto_trigger_check {
            let entity = cx.entity().clone();
            cx.defer(move |cx| {
                entity.update(cx, |this, cx| {
                    this.trigger_update_check(cx);
                });
            });
        }

        view
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

    /// Dispatches an asynchronous GitHub update check on Tokio's runtime.
    pub fn trigger_update_check(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.updater_ui_state,
            UpdaterUiState::Checking | UpdaterUiState::Downloading { .. }
        ) {
            return;
        }

        self.updater_ui_state = UpdaterUiState::Checking;
        self.status_text = "Checking for updates...".to_string();
        cx.notify();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UpdateCheckResult>();
        let client = GitHubUpdaterClient::from_config(&self.config);

        tokio_runtime().spawn(async move {
            let res = client.check_for_updates().await;
            let _ = tx.send(res);
        });

        cx.spawn(async move |this, cx| {
            if let Some(check_result) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    match check_result.status {
                        UpdateStatus::UpToDate => {
                            this.updater_ui_state = UpdaterUiState::UpToDate {
                                version: check_result.current_version.clone(),
                            };
                            this.status_text = format!(
                                "WinLook v{} is up to date",
                                check_result.current_version
                            );
                        }
                        UpdateStatus::Available(release) => {
                            this.updater_ui_state = UpdaterUiState::Available {
                                release: Box::new(release),
                                target_asset: check_result.target_asset,
                            };
                            this.status_text = "New WinLook update is available!".to_string();
                        }
                        UpdateStatus::RateLimited(reset_time) => {
                            let msg = format_rate_limit_msg(reset_time);
                            this.updater_ui_state = UpdaterUiState::Error(msg.clone());
                            this.status_text = msg;
                        }
                        UpdateStatus::Failed(err) => {
                            this.updater_ui_state = UpdaterUiState::Error(err.clone());
                            this.status_text = format!("Update check failed: {}", err);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Dispatches streaming download of the release asset and launches the installer.
    pub fn trigger_download_and_install(&mut self, asset: ReleaseAsset, cx: &mut Context<Self>) {
        self.updater_ui_state = UpdaterUiState::Downloading {
            downloaded_bytes: 0,
            total_bytes: Some(asset.size),
            percent: 0.0,
        };
        self.status_text = format!("Downloading {}...", asset.name);
        cx.notify();

        let (progress_tx, mut progress_rx) =
            tokio::sync::mpsc::unbounded_channel::<DownloadProgress>();
        let (done_tx, mut done_rx) =
            tokio::sync::mpsc::unbounded_channel::<Result<std::path::PathBuf, String>>();

        tokio_runtime().spawn(async move {
            let res = crate::updater::download_asset(&asset, Some(progress_tx))
                .await
                .map_err(|e| format!("{:#}", e));
            let _ = done_tx.send(res);
        });

        // 1. Progress consumer
        cx.spawn(async move |this, cx| {
            while let Some(prog) = progress_rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    if let UpdaterUiState::Downloading {
                        ref mut downloaded_bytes,
                        ref mut total_bytes,
                        ref mut percent,
                    } = this.updater_ui_state
                    {
                        *downloaded_bytes = prog.downloaded_bytes;
                        *total_bytes = prog.total_bytes;
                        *percent = prog.percent.unwrap_or(0.0);
                        cx.notify();
                    }
                });
            }
        })
        .detach();

        // 2. Completion consumer & process execution
        cx.spawn(async move |this, cx| {
            if let Some(res) = done_rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    match res {
                        Ok(installer_path) => {
                            this.updater_ui_state = UpdaterUiState::Installing;
                            this.status_text = "Starting installer...".to_string();
                            cx.notify();

                            match crate::updater::execute_installer(&installer_path) {
                                Ok(()) => {
                                    // Quit cleanly so winlook.exe file locks are released
                                    cx.quit();
                                }
                                Err(e) => {
                                    this.updater_ui_state = UpdaterUiState::Error(format!(
                                        "Failed to launch installer: {}",
                                        e
                                    ));
                                    this.status_text = format!("Installer error: {}", e);
                                    cx.notify();
                                }
                            }
                        }
                        Err(e) => {
                            this.updater_ui_state =
                                UpdaterUiState::Error(format!("Download failed: {}", e));
                            this.status_text = format!("Download error: {}", e);
                            cx.notify();
                        }
                    }
                });
            }
        })
        .detach();
    }

    /// Renders the inner card contents based on UpdaterUiState.
    fn render_updater_card(&self, cx: &Context<Self>) -> impl IntoElement {
        let card_body = match &self.updater_ui_state {
            UpdaterUiState::Idle => {
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_SUB))
                            .child("WinLook is ready to check for updates."),
                    )
                    .child(tool_btn(
                        "btn-check-updates",
                        Some(IconName::RefreshCw),
                        "Check for Updates",
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.trigger_update_check(cx);
                        }),
                    ))
            }
            UpdaterUiState::Checking => {
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Spinner::new().small())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_MAIN))
                                    .child("Checking GitHub for the latest release..."),
                            ),
                    )
                    .child(tool_btn(
                        "btn-check-updates-busy",
                        Some(IconName::RefreshCw),
                        "Checking...",
                        true,
                        |_, _, _| {},
                    ))
            }
            UpdaterUiState::UpToDate { version } => {
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Icon::new(IconName::CircleCheck)
                                    .xsmall()
                                    .text_color(rgb(0x4ade80)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_MAIN))
                                    .child(format!("WinLook v{} is up to date.", version)),
                            ),
                    )
                    .child(tool_btn(
                        "btn-check-again",
                        Some(IconName::RefreshCw),
                        "Check Again",
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.trigger_update_check(cx);
                        }),
                    ))
            }
            UpdaterUiState::Available {
                release,
                target_asset,
            } => {
                let asset_opt = target_asset.clone();
                let is_expanded = self.notes_expanded;
                let release_url = release.html_url.clone();

                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Icon::new(IconName::TriangleAlert)
                                            .xsmall()
                                            .text_color(rgb(0xfbbf24)),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(TEXT_MAIN))
                                            .child(format!(
                                                "Update Available: {}",
                                                release.tag_name
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_DIM))
                                    .child(format_published_date(&release.published_at)),
                            ),
                    )
                    .when_some(release.name.as_ref(), |el, name| {
                        el.child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(TEXT_SUB))
                                .child(name.clone()),
                        )
                    })
                    .when_some(target_asset.as_ref(), |el, asset| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(rgb(TEXT_DIM))
                                .child(format!(
                                    "Installer asset: {} ({:.1} MB)",
                                    asset.name,
                                    asset.size as f64 / 1_048_576.0
                                )),
                        )
                    })
                    // Collapsible Release Notes Header
                    .child(
                        h_flex()
                            .id("btn-toggle-release-notes")
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .child(
                                Icon::new(if is_expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .xsmall()
                                .text_color(rgb(TEXT_SUB)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(TEXT_MAIN))
                                    .child(if is_expanded {
                                        "Hide Release Notes"
                                    } else {
                                        "Show Release Notes"
                                    }),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.notes_expanded = !this.notes_expanded;
                                cx.notify();
                            })),
                    )
                    // Expanded Bounded Scrollable Release Notes
                    .when(is_expanded, |el| {
                        el.child(
                            div()
                                .w_full()
                                .max_h(px(130.0))
                                .overflow_y_scrollbar()
                                .p_2()
                                .rounded(px(3.0))
                                .bg(rgb(BG_INPUT))
                                .border_1()
                                .border_color(rgb(BORDER_LINE))
                                .text_xs()
                                .text_color(rgb(TEXT_SUB))
                                .child(
                                    release
                                        .body
                                        .as_deref()
                                        .filter(|b| !b.trim().is_empty())
                                        .unwrap_or("No release notes provided.")
                                        .to_string(),
                                ),
                        )
                    })
                    // Actions Row
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .when_some(asset_opt, |el, asset| {
                                el.child(tool_btn(
                                    "btn-download-install",
                                    Some(IconName::ArrowDown),
                                    "Download & Install",
                                    true,
                                    cx.listener(move |this, _, _, cx| {
                                        this.trigger_download_and_install(asset.clone(), cx);
                                    }),
                                ))
                            })
                            .child(tool_btn(
                                "btn-view-github-release",
                                Some(IconName::ExternalLink),
                                "View on GitHub",
                                false,
                                move |_, _, _| {
                                    let _ = open::that(&release_url);
                                },
                            ))
                            .child(tool_btn(
                                "btn-recheck-updates",
                                Some(IconName::RefreshCw),
                                "Re-check",
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.trigger_update_check(cx);
                                }),
                            )),
                    )
            }
            UpdaterUiState::Downloading {
                downloaded_bytes,
                total_bytes,
                percent,
            } => {
                let clamped = (*percent / 100.0).clamp(0.0, 1.0);
                v_flex()
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
                                    .child(format!("Downloading update... {:.0}%", percent)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_DIM))
                                    .child(format_bytes_progress(*downloaded_bytes, *total_bytes)),
                            ),
                    )
                    // Zero-dependency native GPUI Progress Bar
                    .child(
                        div()
                            .w_full()
                            .h(px(6.0))
                            .bg(rgb(BG_INPUT))
                            .rounded(px(3.0))
                            .border_1()
                            .border_color(rgb(BORDER_LINE))
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(clamped))
                                    .bg(rgb(0x3b82f6))
                                    .rounded(px(2.0)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_DIM))
                            .child(
                                "Downloading installer from GitHub. WinLook will restart once complete.",
                            ),
                    )
            }
            UpdaterUiState::Installing => {
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Spinner::new().small())
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_MAIN))
                            .child("Download finished. Starting installer and closing WinLook..."),
                    )
            }
            UpdaterUiState::Error(err) => {
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Icon::new(IconName::CircleAlert)
                                    .xsmall()
                                    .text_color(rgb(0xf87171)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xf87171))
                                    .child(err.clone()),
                            ),
                    )
                    .child(tool_btn(
                        "btn-retry-check",
                        Some(IconName::RefreshCw),
                        "Try Again",
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.trigger_update_check(cx);
                        }),
                    ))
            }
        };

        div()
            .w_full()
            .p_3()
            .rounded(px(3.0))
            .bg(rgb(BG_SURFACE))
            .border_1()
            .border_color(rgb(BORDER_LINE))
            .child(card_body)
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
                            )
                            // ================================================================
                            // SECTION 6: APPLICATION UPDATES
                            // ================================================================
                            .child(
                                v_flex()
                                    .py_2p5()
                                    .gap_2()
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(rgb(TEXT_MAIN))
                                                    .child("Application Updates"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(TEXT_SUB))
                                                    .child(format!("WinLook v{}", env!("CARGO_PKG_VERSION"))),
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
                                                    .child("Automatically check for updates on startup"),
                                            )
                                            .child(
                                                Switch::new("sw-check-updates-startup")
                                                    .small()
                                                    .checked(self.config.check_updates_on_startup)
                                                    .on_click(cx.listener(|this, checked, _, cx| {
                                                        this.config.check_updates_on_startup = *checked;
                                                        let _ = this.config.save();
                                                        this.status_text = if *checked {
                                                            "Startup update checks enabled".to_string()
                                                        } else {
                                                            "Startup update checks disabled".to_string()
                                                        };
                                                        cx.notify();
                                                    })),
                                            ),
                                    )
                                    .child(self.render_updater_card(cx)),
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

fn format_rate_limit_msg(reset_epoch_secs: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if reset_epoch_secs > now_secs {
        let diff_mins = (reset_epoch_secs - now_secs).div_ceil(60);
        format!(
            "GitHub rate limit reached. Resets in ~{} min (HTTP 403)",
            diff_mins
        )
    } else {
        "GitHub rate limit reached. Please try again shortly.".to_string()
    }
}

fn format_bytes_progress(downloaded: u64, total: Option<u64>) -> String {
    let dl_mb = downloaded as f64 / 1_048_576.0;
    if let Some(tot) = total {
        let tot_mb = tot as f64 / 1_048_576.0;
        format!("{:.1} MB / {:.1} MB", dl_mb, tot_mb)
    } else {
        format!("{:.1} MB", dl_mb)
    }
}

fn format_published_date(published_at: &Option<String>) -> String {
    match published_at {
        Some(iso) => {
            let date_part = iso.split('T').next().unwrap_or(iso.as_str());
            format!("Released: {}", date_part)
        }
        None => String::new(),
    }
}
