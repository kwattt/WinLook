#![windows_subsystem = "windows"]

pub mod capture;
pub mod config;
pub mod ocr;
pub mod services;
pub mod ui;
pub mod updater;

use anyhow::Result;
use config::AppConfig;
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use gpui::QuitMode;
use std::str::FromStr;
use tokio::sync::mpsc;
use tray_icon::{
    Icon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

#[derive(Debug, Clone)]
enum DaemonCommand {
    CaptureRegion,
    TakeFullscreenScreenshot,
    OpenScreenshotsFolder,
    OpenSettings,
    CheckForUpdates,
    ReloadHotkey(String),
    Quit,
}

/// Generates a clean 32x32 neutral dark/white crop-frame icon for the Windows System Tray.
fn build_tray_icon() -> Result<Icon> {
    let size = 32u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];

    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            if (2..=29).contains(&x) && (2..=29).contains(&y) {
                // Neutral dark matte background (#202020)
                rgba[idx] = 32;
                rgba[idx + 1] = 32;
                rgba[idx + 2] = 32;
                rgba[idx + 3] = 255;

                // Subtle outer border (#444444)
                if x == 2 || x == 29 || y == 2 || y == 29 {
                    rgba[idx] = 68;
                    rgba[idx + 1] = 68;
                    rgba[idx + 2] = 68;
                }

                // Crisp inner crop corners (#EAEAEA)
                let corner_h = ((7..=12).contains(&x) || (19..=24).contains(&x))
                    && ((7..=8).contains(&y) || (23..=24).contains(&y));
                let corner_v = ((7..=8).contains(&x) || (23..=24).contains(&x))
                    && ((7..=12).contains(&y) || (19..=24).contains(&y));
                if corner_h || corner_v {
                    rgba[idx] = 234;
                    rgba[idx + 1] = 234;
                    rgba[idx + 2] = 234;
                }
            }
        }
    }

    Ok(Icon::from_rgba(rgba, size, size)?)
}

fn parse_hotkey(hotkey_str: &str) -> HotKey {
    HotKey::from_str(hotkey_str)
        .unwrap_or_else(|_| HotKey::new(Some(Modifiers::ALT), Code::Space))
}

fn close_all_windows(cx: &mut gpui::App) {
    for window in cx.windows() {
        let _ = window.update(cx, |_, window, _| {
            window.remove_window();
        });
    }
}

fn trigger_capture(cx: &mut gpui::App, tx: mpsc::UnboundedSender<DaemonCommand>) {
    close_all_windows(cx);

    let config = AppConfig::load();
    match capture::capture_virtual_screen() {
        Ok(frame) => {
            if let Err(err) = ui::open_overlay_window(cx, frame, config, move |new_hk| {
                let _ = tx.send(DaemonCommand::ReloadHotkey(new_hk));
            }) {
                eprintln!("Failed to open WinLook overlay window: {:#}", err);
            }
        }
        Err(err) => {
            eprintln!("Screen capture failed: {:#}", err);
        }
    }
}

fn trigger_fullscreen_screenshot() {
    let config = AppConfig::load();
    match capture::capture_virtual_screen() {
        Ok(frame) => {
            let rect = frame.full_rect();
            match frame.save_crop_to_disk(rect, &config) {
                Ok(path) => {
                    println!("Saved fullscreen screenshot to {}", path.display());
                }
                Err(err) => {
                    eprintln!("Failed to save fullscreen screenshot: {:#}", err);
                }
            }
        }
        Err(err) => {
            eprintln!("Fullscreen capture failed: {:#}", err);
        }
    }
}

fn trigger_open_settings(cx: &mut gpui::App, tx: mpsc::UnboundedSender<DaemonCommand>) {
    close_all_windows(cx);
    let tx_cap = tx.clone();
    let tx_hk = tx;
    if let Err(err) = ui::open_settings_window(
        cx,
        move |cx| {
            trigger_capture(cx, tx_cap.clone());
        },
        move |new_hk| {
            let _ = tx_hk.send(DaemonCommand::ReloadHotkey(new_hk));
        },
    ) {
        eprintln!("Failed to open WinLook Preferences: {:#}", err);
    }
}

fn main() -> Result<()> {
    // Enable Windows Per-Monitor DPI awareness V2 early before GPUI or window initialization
    capture::init_dpi_awareness();

    // Immediately heal any desktop windows that may have had WS_POPUP + WS_EX_TOPMOST applied
    ui::restore_accidentally_locked_windows();

    let config = AppConfig::load();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--fix-windows") {
        return Ok(());
    }

    let capture_on_start = args.iter().any(|arg| arg == "--capture" || arg == "--now");
    let silent_start = args.iter().any(|arg| arg == "--silent" || arg == "--background");

    println!(
        "WinLook — Shortcut: {} | Model: {} | Save Folder: {}",
        config.hotkey, config.deepseek_model, config.screenshot_dir
    );

    let app = gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .with_quit_mode(QuitMode::Explicit);

    app.run(move |cx| {
        gpui_kit::init(cx);
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);

        let (tx, mut rx) = mpsc::unbounded_channel::<DaemonCommand>();

        // 1. Register Global Hotkey
        let hotkey_manager = GlobalHotKeyManager::new().ok();
        let mut active_registered_hotkey = parse_hotkey(&config.hotkey);

        if let Some(ref mgr) = hotkey_manager {
            if let Err(err) = mgr.register(active_registered_hotkey) {
                eprintln!(
                    "Note: Could not register '{}' ({}), registering fallback 'Alt+Shift+A'",
                    config.hotkey, err
                );
                let fallback = HotKey::new(Some(Modifiers::ALT | Modifiers::SHIFT), Code::KeyA);
                if mgr.register(fallback).is_ok() {
                    active_registered_hotkey = fallback;
                }
            }
        }

        let tx_hotkey = tx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state == HotKeyState::Pressed {
                let _ = tx_hotkey.send(DaemonCommand::CaptureRegion);
            }
        }));

        // 2. Initialize System Tray Icon & Context Menu
        let tray_menu = Menu::new();
        let item_capture = MenuItem::new(
            format!("Select Region ({})", config.hotkey),
            true,
            None,
        );
        let item_screenshot = MenuItem::new("Fullscreen Screenshot", true, None);
        let item_open_folder = MenuItem::new("Open Screenshots Folder", true, None);
        let item_check_updates = MenuItem::new("Check for Updates...", true, None);
        let item_settings = MenuItem::new("Preferences...", true, None);
        let item_quit = MenuItem::new("Quit WinLook", true, None);

        let capture_id = item_capture.id().clone();
        let screenshot_id = item_screenshot.id().clone();
        let open_folder_id = item_open_folder.id().clone();
        let check_updates_id = item_check_updates.id().clone();
        let settings_id = item_settings.id().clone();
        let quit_id = item_quit.id().clone();

        let _ = tray_menu.append(&item_capture);
        let _ = tray_menu.append(&item_screenshot);
        let _ = tray_menu.append(&item_open_folder);
        let _ = tray_menu.append(&PredefinedMenuItem::separator());
        let _ = tray_menu.append(&item_check_updates);
        let _ = tray_menu.append(&item_settings);
        let _ = tray_menu.append(&PredefinedMenuItem::separator());
        let _ = tray_menu.append(&item_quit);

        let tray_icon = build_tray_icon().ok().and_then(|icon| {
            TrayIconBuilder::new()
                .with_menu(Box::new(tray_menu))
                .with_tooltip(format!("WinLook ({})", config.hotkey))
                .with_icon(icon)
                .build()
                .ok()
        });

        let tx_tray = tx.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::DoubleClick { .. } = event {
                let _ = tx_tray.send(DaemonCommand::OpenSettings);
            }
        }));

        let tx_menu = tx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == capture_id {
                let _ = tx_menu.send(DaemonCommand::CaptureRegion);
            } else if event.id == screenshot_id {
                let _ = tx_menu.send(DaemonCommand::TakeFullscreenScreenshot);
            } else if event.id == open_folder_id {
                let _ = tx_menu.send(DaemonCommand::OpenScreenshotsFolder);
            } else if event.id == check_updates_id {
                let _ = tx_menu.send(DaemonCommand::CheckForUpdates);
            } else if event.id == settings_id {
                let _ = tx_menu.send(DaemonCommand::OpenSettings);
            } else if event.id == quit_id {
                let _ = tx_menu.send(DaemonCommand::Quit);
            }
        }));

        // Background startup update check if configured
        if config.check_updates_on_startup {
            let tx_startup = tx.clone();
            let cfg_clone = config.clone();
            crate::ui::overlay::tokio_runtime().spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                let client = crate::updater::GitHubUpdaterClient::from_config(&cfg_clone);
                let res = client.check_for_updates().await;
                if let crate::updater::UpdateStatus::Available(_) = &res.status {
                    crate::ui::settings::set_pending_update_result(res);
                    let _ = tx_startup.send(DaemonCommand::OpenSettings);
                }
            });
        }

        // 3. Startup Window Behavior
        if capture_on_start {
            trigger_capture(cx, tx.clone());
        } else if !silent_start {
            trigger_open_settings(cx, tx.clone());
        }

        // 4. Listen for Global Hotkey, Hot-Reload, & Tray Menu events
        let tx_loop = tx.clone();
        cx.spawn(async move |cx| {
            let _keep_tray = tray_icon;

            while let Some(cmd) = rx.recv().await {
                match cmd {
                    DaemonCommand::CaptureRegion => {
                        let tx_c = tx_loop.clone();
                        cx.update(|cx| {
                            trigger_capture(cx, tx_c);
                        });
                    }
                    DaemonCommand::TakeFullscreenScreenshot => {
                        trigger_fullscreen_screenshot();
                    }
                    DaemonCommand::OpenScreenshotsFolder => {
                        let cfg = AppConfig::load();
                        if let Ok(dir) = cfg.ensure_screenshots_dir() {
                            let _ = open::that(dir);
                        }
                    }
                    DaemonCommand::OpenSettings => {
                        let tx_s = tx_loop.clone();
                        cx.update(|cx| {
                            trigger_open_settings(cx, tx_s);
                        });
                    }
                    DaemonCommand::CheckForUpdates => {
                        crate::ui::settings::request_update_check_on_next_open();
                        let tx_s = tx_loop.clone();
                        cx.update(|cx| {
                            trigger_open_settings(cx, tx_s);
                        });
                    }
                    DaemonCommand::ReloadHotkey(new_hk_str) => {
                        if let Some(ref mgr) = hotkey_manager {
                            let _ = mgr.unregister(active_registered_hotkey);
                            if !new_hk_str.trim().is_empty() {
                                if let Ok(new_hk) = HotKey::from_str(&new_hk_str) {
                                    if mgr.register(new_hk).is_ok() {
                                        active_registered_hotkey = new_hk;
                                    }
                                }
                            }
                        }
                    }
                    DaemonCommand::Quit => {
                        cx.update(|cx| {
                            cx.quit();
                        });
                        break;
                    }
                }
            }
        })
        .detach();
    });

    Ok(())
}
