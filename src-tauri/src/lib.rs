#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use active_win_pos_rs::get_active_window;
use base64::{engine::general_purpose, Engine as _};
use screenshots::Screen;
use serde::Serialize;
use std::path::PathBuf;
use tauri::{Emitter, Manager};
use tokio::time::{sleep, Duration};

#[derive(Serialize, Clone)]
struct CaptureResponse {
    success: bool,
    image_base64: String,
    file_path: String,
    message: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
struct WatchContext {
    source_app: String,
    window_title: String,
    matched: bool,
    matched_rule: String,
}

fn get_active_context() -> WatchContext {
    match get_active_window() {
        Ok(window) => {
            let app_name = if window.app_name.is_empty() {
                "unknown".to_string()
            } else {
                window.app_name
            };

            let title = if window.title.is_empty() {
                "unknown".to_string()
            } else {
                window.title
            };

            let app_l = app_name.to_lowercase();
            let title_l = title.to_lowercase();

            let (matched, matched_rule) = if app_l.contains("ccc")
                || title_l.contains("ccc")
                || title_l.contains("estimate")
            {
                (true, "ccc_estimate".to_string())
            } else if title_l.contains("supp") || title_l.contains("supplement") {
                (true, "ccc_supplement".to_string())
            } else if app_l.contains("mitchell") || title_l.contains("mitchell") {
                (true, "mitchell".to_string())
            } else if title_l.contains("repair order") || title_l.contains("ro ") {
                (true, "repair_order".to_string())
            } else {
                (false, "none".to_string())
            };

            WatchContext {
                source_app: app_name,
                window_title: title,
                matched,
                matched_rule,
            }
        }
        Err(_) => WatchContext {
            source_app: "unknown".to_string(),
            window_title: "unknown".to_string(),
            matched: false,
            matched_rule: "none".to_string(),
        },
    }
}

#[tauri::command]
fn get_watch_context() -> WatchContext {
    get_active_context()
}

#[tauri::command]
async fn capture_screen() -> Result<CaptureResponse, String> {
    let temp_dir = std::env::temp_dir();
    let file_path: PathBuf = temp_dir.join("fw_capture.png");

    let screens = Screen::all().map_err(|e| e.to_string())?;
    let screen = screens.get(0).ok_or("No screen found")?;

    let image = screen.capture().map_err(|e| e.to_string())?;
    image.save(&file_path).map_err(|e| e.to_string())?;

    let context = get_active_context();

    let image_bytes = std::fs::read(&file_path).map_err(|e| e.to_string())?;
    let image_base64 = general_purpose::STANDARD.encode(image_bytes);

    Ok(CaptureResponse {
        success: true,
        image_base64,
        file_path: file_path.to_string_lossy().to_string(),
        message: format!(
            "Capture sent successfully.\nsource_app: {}\nwindow_title: {}\nmatched_rule: {}",
            context.source_app, context.window_title, context.matched_rule
        ),
    })
}

#[tauri::command]
fn show_overlay(window: tauri::Window) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn hide_overlay(window: tauri::Window) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())?;
    Ok(())
}

fn start_watcher(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last_rule = String::new();
        let mut last_title = String::new();

        loop {
            let ctx = get_active_context();

            let changed = ctx.matched_rule != last_rule || ctx.window_title != last_title;

            if changed {
                let _ = app.emit("watch-context-changed", ctx.clone());

                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_always_on_top(true);
                }

                last_rule = ctx.matched_rule.clone();
                last_title = ctx.window_title.clone();
            }

            sleep(Duration::from_millis(1500)).await;
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_always_on_top(true);
                let _ = window.set_skip_taskbar(true);
                let _ = window.set_resizable(false);
            }

            start_watcher(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_watch_context,
            capture_screen,
            show_overlay,
            hide_overlay
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}