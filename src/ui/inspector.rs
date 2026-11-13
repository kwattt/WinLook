use crate::capture::{ScreenRect, copy_text_to_clipboard};
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

/// Computes the clamped floating toolbar position (anchor_x, anchor_y)
/// constrained within the active monitor's work area, preventing rendering in dead zones.
#[allow(clippy::too_many_arguments)]
pub fn calculate_toolbar_position(
    sel: ScreenRect,
    scale: f32,
    dock_width: f32,
    needed_height: f32,
    monitors: &[crate::capture::MonitorInfo],
    vx: i32,
    vy: i32,
    fallback_lw: f32,
    fallback_lh: f32,
) -> (f32, f32) {
    let s = if scale <= 0.0 { 1.0 } else { scale };
    let (sx, sy, _sw, sh) = sel.to_logical(s);

    // Find the monitor that has the greatest intersection with the selection,
    // or contains the selection center / top-left, or fallback to primary monitor.
    let active_monitor = monitors
        .iter()
        .max_by_key(|m| {
            let buf_rect = m.buffer_rect(vx, vy);
            buf_rect.intersection_area(&sel)
        })
        .filter(|m| {
            let buf_rect = m.buffer_rect(vx, vy);
            buf_rect.intersects(&sel)
        })
        .or_else(|| {
            let center_x = sel.x + (sel.width / 2) as i32;
            let center_y = sel.y + (sel.height / 2) as i32;
            monitors.iter().find(|m| m.contains_buffer_point(center_x, center_y, vx, vy))
        })
        .or_else(|| monitors.iter().find(|m| m.is_primary))
        .or_else(|| monitors.first());

    let (min_x, max_x, min_y, max_y) = if let Some(m) = active_monitor {
        let work = m.buffer_work_rect(vx, vy);
        let (wx, wy, ww, wh) = work.to_logical(s);
        (wx, wx + ww, wy, wy + wh)
    } else {
        (0.0, fallback_lw, 0.0, fallback_lh)
    };

    let anchor_x = sx.clamp(min_x + 12.0, (max_x - dock_width - 12.0).max(min_x + 12.0));

    let space_below = max_y - (sy + sh);
    let anchor_y = if space_below >= needed_height + 12.0 {
        sy + sh + 6.0
    } else if sy - min_y >= needed_height + 36.0 {
        (sy - needed_height - 6.0).max(min_y + 36.0)
    } else {
        (max_y - needed_height - 12.0).max(min_y + 36.0)
    };

    (anchor_x, anchor_y)
}

/// Renders the selection toolbar, automatic OCR text readout, and DeepSeek response panel.
pub fn render_floating_inspector(
    view: &OverlayView,
    cx: &mut Context<OverlayView>,
) -> impl IntoElement {
    let sel = view.effective_selection();
    let scale = view.frame.scale_factor;
    let lw = view.frame.logical_width();
    let lh = view.frame.logical_height();

    let extracted_text = view.state.auto_ocr_text.clone();
    let has_ocr_text = !extracted_text.trim().is_empty();
    let has_ai_key = crate::config::AppConfig::has_deepseek_api_key();
    let reasoning_on = view.config.enable_ai_reasoning;

    let dock_width = if has_ai_key { 640.0_f32 } else { 360.0_f32 };

    let has_ai_drawer = view.state.mode == OverlayMode::AskDeepSeek
        && (!view.state.ai_markdown_response.is_empty() || view.state.is_loading);
    let show_ocr_drawer = view.state.show_ocr_panel && has_ocr_text && !has_ai_drawer;

    let needed_height = if has_ai_drawer {
        280.0
    } else if show_ocr_drawer {
        175.0
    } else {
        44.0
    };

    let (anchor_x, anchor_y) = calculate_toolbar_position(
        sel,
        scale,
        dock_width,
        needed_height,
        &view.frame.monitors,
        view.frame.vx,
        view.frame.vy,
        lw,
        lh,
    );

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
                                crate::ui::dismiss_overlay(window, cx);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{MonitorInfo, ScreenRect};

    #[test]
    fn test_inspector_toolbar_clamping_multi_monitor() {
        // Topology: Primary monitor (1920x1080) at (0, 0)
        // Secondary monitor (1920x1080) at (-1920, 0)
        // Virtual desktop: vx = -1920, vy = 0, vw = 3840, vh = 1080
        let vx = -1920;
        let vy = 0;
        let scale = 1.0;
        let dock_width = 360.0;
        let needed_height = 44.0;

        let monitors = vec![
            MonitorInfo {
                display_id: 0,
                rect: ScreenRect { x: -1920, y: 0, width: 1920, height: 1080 },
                work_rect: ScreenRect { x: -1920, y: 0, width: 1920, height: 1040 }, // 40px taskbar at bottom
                is_primary: false,
                scale_factor: 1.0,
            },
            MonitorInfo {
                display_id: 0,
                rect: ScreenRect { x: 0, y: 0, width: 1920, height: 1080 },
                work_rect: ScreenRect { x: 0, y: 0, width: 1920, height: 1040 },
                is_primary: true,
                scale_factor: 1.0,
            },
        ];

        // Selection completely on secondary monitor: in buffer coordinates [100, 100, 400, 300]
        // (maps to virtual [-1820, 100, 400, 300])
        let sel_sec = ScreenRect { x: 100, y: 100, width: 400, height: 300 };
        let (ax, ay) = calculate_toolbar_position(
            sel_sec,
            scale,
            dock_width,
            needed_height,
            &monitors,
            vx,
            vy,
            3840.0,
            1080.0,
        );
        // Secondary buffer work area is [0..1920, 0..1040]
        assert!(ax >= 12.0 && ax + dock_width <= 1920.0 - 12.0);
        assert_eq!(ax, 100.0);
        assert_eq!(ay, 406.0); // 100 + 300 + 6 = 406

        // Selection near bottom of secondary monitor, toolbar should fit above or inside work area (not in taskbar)
        let sel_bot = ScreenRect { x: 100, y: 800, width: 400, height: 230 };
        let (_ax2, ay2) = calculate_toolbar_position(
            sel_bot,
            scale,
            dock_width,
            needed_height,
            &monitors,
            vx,
            vy,
            3840.0,
            1080.0,
        );
        // Space below is 1040 - (800 + 230) = 10, not enough for 44 + 12 = 56
        // So it flips above: 800 - 44 - 6 = 750
        assert_eq!(ay2, 750.0);

        // Selection on primary monitor: in buffer coordinates [2000, 100, 400, 300]
        // Primary buffer work area is [1920..3840, 0..1040]
        let sel_prim = ScreenRect { x: 2000, y: 100, width: 400, height: 300 };
        let (p_ax, _p_ay) = calculate_toolbar_position(
            sel_prim,
            scale,
            dock_width,
            needed_height,
            &monitors,
            vx,
            vy,
            3840.0,
            1080.0,
        );
        assert!(p_ax >= 1920.0 + 12.0 && p_ax + dock_width <= 3840.0 - 12.0);
        assert_eq!(p_ax, 2000.0);
    }

    #[test]
    fn test_inspector_toolbar_clamping_l_shaped_avoid_dead_zone() {
        // Topology: L-shaped setup
        // Monitor 1 (primary): 1920x1080 at (0, 0)
        // Monitor 2 (vertical secondary): 1080x1920 at (1920, 0)
        // Virtual desktop: vx = 0, vy = 0, vw = 3000, vh = 1920
        // Dead zone is at [0..1920, 1080..1920] (below monitor 1)
        let vx = 0;
        let vy = 0;
        let scale = 1.0;
        let dock_width = 360.0;
        let needed_height = 50.0;

        let monitors = vec![
            MonitorInfo {
                display_id: 0,
                rect: ScreenRect { x: 0, y: 0, width: 1920, height: 1080 },
                work_rect: ScreenRect { x: 0, y: 0, width: 1920, height: 1040 },
                is_primary: true,
                scale_factor: 1.0,
            },
            MonitorInfo {
                display_id: 0,
                rect: ScreenRect { x: 1920, y: 0, width: 1080, height: 1920 },
                work_rect: ScreenRect { x: 1920, y: 0, width: 1080, height: 1880 },
                is_primary: false,
                scale_factor: 1.0,
            },
        ];

        // Selection near bottom of monitor 1: sy = 1000, sh = 35 -> bottom = 1035
        // Global virtual lh is 1920, so unconstrained clamping would think space_below = 1920 - 1035 = 885
        // and place toolbar at 1041, which is in the dead zone!
        // But with monitor work area clamping (max_y = 1040), space_below = 1040 - 1035 = 5.
        // It flips above: 1000 - 50 - 6 = 944!
        let sel = ScreenRect { x: 500, y: 1000, width: 300, height: 35 };
        let (ax, ay) = calculate_toolbar_position(
            sel,
            scale,
            dock_width,
            needed_height,
            &monitors,
            vx,
            vy,
            3000.0,
            1920.0,
        );
        assert_eq!(ax, 500.0);
        assert_eq!(ay, 944.0);
        assert!(ay + needed_height <= 1040.0);
    }
}
