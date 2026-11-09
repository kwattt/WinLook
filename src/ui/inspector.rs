use crate::capture::copy_text_to_clipboard;
use crate::ui::overlay::OverlayView;
use crate::ui::{
    BG_INPUT, BG_SURFACE, BG_WINDOW, BORDER_LINE, OverlayMode, TEXT_DIM, TEXT_MAIN, TEXT_SUB,
    icon_btn, tool_btn,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, div, px, rgb,
};
use gpui_component::IconName;
use gpui_component::input::Input;
use gpui_component::text::TextView;
use gpui_component::{h_flex, v_flex};

/// Renders the selection toolbar, automatic OCR text readout, and DeepSeek response panel.
pub fn render_floating_inspector(
    view: &OverlayView,
    cx: &mut Context<OverlayView>,
) -> impl IntoElement {
    let sel = view.effective_selection();
    let scale = view.frame.scale_factor;
    let lw = view.frame.logical_width();
    let lh = view.frame.logical_height();

    let (sx, sy, _sw, sh) = sel.to_logical(scale);

    let extracted_text = view.state.auto_ocr_text.clone();
    let has_ocr_text = !extracted_text.trim().is_empty();
    let has_ai_key = crate::config::AppConfig::has_deepseek_api_key();
    let reasoning_on = view.config.enable_ai_reasoning;

    let dock_width = if has_ai_key { 640.0_f32 } else { 360.0_f32 };
    let anchor_x = sx.clamp(12.0, (lw - dock_width - 12.0).max(12.0));

    let has_ai_drawer = view.state.mode == OverlayMode::AskDeepSeek
        && (!view.state.ai_markdown_response.is_empty() || view.state.is_loading);
    let show_ocr_drawer = view.state.show_ocr_panel && has_ocr_text && !has_ai_drawer;

    let space_below = lh - (sy + sh);
    let needed_height = if has_ai_drawer {
        280.0
    } else if show_ocr_drawer {
        175.0
    } else {
        44.0
    };

    let anchor_y = if space_below >= needed_height + 12.0 {
        sy + sh + 6.0
    } else if sy >= needed_height + 36.0 {
        (sy - needed_height - 6.0).max(36.0)
    } else {
        (lh - needed_height - 12.0).max(36.0)
    };

    let is_translating = view.state.mode == OverlayMode::InPlaceTranslate;
    let is_asking = view.state.is_loading && view.state.mode == OverlayMode::AskDeepSeek;
    let response_md = view.state.ai_markdown_response.clone();

    div()
        .id("selection-toolbar-container")
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .absolute()
        .left(px(anchor_x))
        .top(px(anchor_y))
        .child(
            v_flex()
                .w(px(dock_width))
                .gap_1p5()
                // ============================================================
                // 1. FLAT UTILITARIAN TOOLBAR
                // ============================================================
                .child(
                    h_flex()
                        .gap_1()
                        .p_1()
                        .rounded(px(4.0))
                        .bg(rgb(BG_SURFACE))
                        .border_1()
                        .border_color(rgb(BORDER_LINE))
                        .items_center()
                        .child(tool_btn(
                            "tb-copy-img",
                            Some(IconName::Copy),
                            "Copy Image",
                            false,
                            cx.listener(|this, _, window, cx| {
                                this.copy_selected_image(cx);
                                window.remove_window();
                            }),
                        ))
                        .child(tool_btn(
                            "tb-save-png",
                            Some(IconName::HardDrive),
                            "Save",
                            false,
                            cx.listener(|this, _, _, cx| {
                                this.save_selected_screenshot(cx);
                            }),
                        ))
                        // Google Lens: searches image AND closes the region select on the same click
                        .child(tool_btn(
                            "tb-google-lens",
                            Some(IconName::Search),
                            "Lens",
                            false,
                            cx.listener(|this, _, window, cx| {
                                this.trigger_google_lens_and_close(window, cx);
                            }),
                        ))
                        .when(has_ocr_text, |bar| {
                            bar.child(tool_btn(
                                "tb-translate",
                                Some(IconName::Globe),
                                "Translate",
                                is_translating,
                                cx.listener(|this, _, _, cx| {
                                    if this.state.mode == OverlayMode::InPlaceTranslate {
                                        this.state.mode = OverlayMode::RegionSelect;
                                        cx.notify();
                                    } else {
                                        this.trigger_in_place_translation(None, cx);
                                    }
                                }),
                            ))
                        })
                        // Only show AI Ask controls when DeepSeek API Key is configured
                        .when(has_ai_key, |bar| {
                            bar.child(div().w(px(1.0)).h(px(18.0)).mx_0p5().bg(rgb(BORDER_LINE)))
                                .child(tool_btn(
                                    "tb-toggle-reasoning",
                                    Some(IconName::Cpu),
                                    if reasoning_on {
                                        "Reason: On"
                                    } else {
                                        "Reason: Off"
                                    },
                                    reasoning_on,
                                    cx.listener(|this, _, _, cx| {
                                        this.config.enable_ai_reasoning =
                                            !this.config.enable_ai_reasoning;
                                        let _ = this.config.save();
                                        cx.notify();
                                    }),
                                ))
                                .child(div().flex_1().child(Input::new(&view.prompt_input)))
                                .child(icon_btn(
                                    "tb-ask-submit",
                                    if is_asking {
                                        IconName::Loader
                                    } else {
                                        IconName::ArrowUp
                                    },
                                    cx.listener(|this, _, window, cx| {
                                        if this.state.is_loading {
                                            return;
                                        }
                                        let text = this.prompt_input.read(cx).value().to_string();
                                        let prompt = if text.trim().is_empty() {
                                            "Explain what is shown in this selection concisely."
                                                .to_string()
                                        } else {
                                            text
                                        };
                                        this.trigger_ask_deepseek(prompt, window, cx);
                                    }),
                                ))
                        }),
                )
                // ============================================================
                // 2. AUTO-OCR PANEL (APPEARS AUTOMATICALLY WHEN TEXT IS IN REGION)
                // ============================================================
                .when(show_ocr_drawer, |stack| {
                    let text_for_copy = extracted_text.clone();
                    stack.child(
                        v_flex()
                            .p_2p5()
                            .gap_1p5()
                            .rounded(px(4.0))
                            .bg(rgb(BG_SURFACE))
                            .border_1()
                            .border_color(rgb(BORDER_LINE))
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(TEXT_SUB))
                                            .child(if view.config.auto_copy_ocr {
                                                "Recognized Text (auto-copied)"
                                            } else {
                                                "Recognized Text"
                                            }),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(tool_btn(
                                                "ocr-copy-btn",
                                                Some(IconName::Copy),
                                                "Copy Text",
                                                false,
                                                cx.listener(move |this, _, _, cx| {
                                                    if copy_text_to_clipboard(&text_for_copy)
                                                        .is_ok()
                                                    {
                                                        this.state.status_message =
                                                            Some("Copied text".to_string());
                                                        cx.notify();
                                                    }
                                                }),
                                            ))
                                            .child(icon_btn(
                                                "close-ocr-panel",
                                                IconName::Close,
                                                cx.listener(|this, _, _, cx| {
                                                    this.state.show_ocr_panel = false;
                                                    cx.notify();
                                                }),
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .id("ocr-text-scroll")
                                    .max_h(px(110.0))
                                    .overflow_y_scroll()
                                    .p_2()
                                    .rounded(px(3.0))
                                    .bg(rgb(BG_INPUT))
                                    .border_1()
                                    .border_color(rgb(BORDER_LINE))
                                    .text_xs()
                                    .text_color(rgb(TEXT_MAIN))
                                    .child(extracted_text),
                            ),
                    )
                })
                // ============================================================
                // 3. STREAMING AI RESPONSE PANEL
                // ============================================================
                .when(has_ai_drawer, |stack| {
                    let resp_copy = response_md.clone();
                    stack.child(
                        v_flex()
                            .p_2p5()
                            .gap_2()
                            .rounded(px(4.0))
                            .bg(rgb(BG_SURFACE))
                            .border_1()
                            .border_color(rgb(BORDER_LINE))
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(TEXT_SUB))
                                            .child(if is_asking && response_md.is_empty() {
                                                format!("DeepSeek ({})...", view.config.deepseek_model)
                                            } else {
                                                view.state.ai_prompt.clone()
                                            }),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .when(!response_md.is_empty(), |row| {
                                                row.child(tool_btn(
                                                    "copy-ai-md",
                                                    Some(IconName::Copy),
                                                    "Copy",
                                                    false,
                                                    cx.listener(move |this, _, _, cx| {
                                                        if copy_text_to_clipboard(&resp_copy)
                                                            .is_ok()
                                                        {
                                                            this.state.status_message = Some(
                                                                "Copied response".to_string(),
                                                            );
                                                            cx.notify();
                                                        }
                                                    }),
                                                ))
                                            })
                                            .child(icon_btn(
                                                "close-ai-drawer",
                                                IconName::Close,
                                                cx.listener(|this, _, _, cx| {
                                                    this.state.mode = OverlayMode::RegionSelect;
                                                    this.state.ai_markdown_response.clear();
                                                    cx.notify();
                                                }),
                                            )),
                                    ),
                            )
                            .when(!response_md.is_empty(), |card| {
                                card.child(
                                    div()
                                        .id("ai-markdown-scroll")
                                        .max_h(px(200.0))
                                        .overflow_y_scroll()
                                        .p_2p5()
                                        .rounded(px(3.0))
                                        .bg(rgb(BG_WINDOW))
                                        .border_1()
                                        .border_color(rgb(BORDER_LINE))
                                        .text_color(rgb(TEXT_MAIN))
                                        .child(
                                            TextView::markdown(
                                                "deepseek-markdown-view",
                                                SharedString::from(response_md),
                                            )
                                            .selectable(true),
                                        ),
                                )
                            })
                            .when(is_asking && view.state.ai_markdown_response.is_empty(), |card| {
                                card.child(
                                    div()
                                        .py_1p5()
                                        .text_xs()
                                        .text_color(rgb(TEXT_DIM))
                                        .child("Streaming response..."),
                                )
                            }),
                    )
                }),
        )
}
