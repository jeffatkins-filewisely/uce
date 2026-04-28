#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::{engine::general_purpose, Engine as _};
mod config;
mod context_rules;
mod context_tracker;
mod memory_store;
mod pdf_watch_config;
mod services;
mod tenant_config;
mod types;
mod uce_webview_url;
mod connection_diagnostics;
mod watch_policy_sync;

use context_rules::{
    all_built_in_rules, candidate_patterns, preferred_capture_mode_for_rule,
    workflow_kind_from_rule_id,
};
use context_tracker::{
    classify_current_context, clear_exclude_rules_for_current_context,
    current_window_info, evaluate_context, exclude_current_context_from_ccc,
    forget_trained_rules_for_current_context, mark_capture, train_current_context_as_ccc,
    train_current_context_for_workflow,
};
use memory_store::load_memory;
use crate::config::print_config;
use screenshots::Screen;
use serde::{Deserialize, Serialize};
use serde_json::json;
use services::printer_check::{PrinterCheckResult, RepairPrinterResult};
use pdf_watch_config::PdfWatchConfig;
use watch_policy_sync::WatchPolicyDocument;
use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconId};
use tauri::webview::{PageLoadEvent, WebviewWindowBuilder};
use tauri::WebviewUrl;
use tauri::{AppHandle, Emitter, Manager, Url};
use tokio::time::sleep;
use types::{RecentMemory, Rule, WatchContext};

#[derive(Serialize, Clone)]
struct CaptureResponse {
    success: bool,
    image_base64: String,
    file_path: String,
    message: String,
    source_app: String,
    window_title: String,
    matched_rule: String,
    changed: bool,
    in_cooldown: bool,
    captured_at_unix_ms: i64,
}

#[derive(Serialize, Clone)]
struct PdfCaptureResponse {
    success: bool,
    image_base64: String,
    file_path: String,
    message: String,
    captured_at_unix_ms: i64,
}

#[derive(Serialize, Clone)]
struct PdfMetaResponse {
    file_path: String,
    modified_unix_ms: i64,
    file_size: u64,
}

#[derive(Serialize)]
struct DebugState {
    context: WatchContext,
    memory: RecentMemory,
    known_rules: Vec<Rule>,
    candidate_patterns: Vec<Rule>,
}

#[derive(Serialize)]
struct LastObservedContext {
    source_app: String,
    window_title: String,
    matched_rule: String,
    workflow_kind: String,
    preferred_capture_mode: String,
    bucket: String,
    in_cooldown: bool,
}

#[derive(Serialize, Deserialize)]
struct WindowPosition {
    x: i32,
    y: i32,
}

fn get_state_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir error: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create app data dir error: {e}"))?;
    // v5: bump when default corner placement changes (ignore stale saved coords).
    Ok(dir.join("window-position-v5.json"))
}

/// Prefer the monitor whose **right edge** is farthest right on the virtual desktop (multi-monitor friendly).
fn monitor_for_top_right_corner(window: &tauri::WebviewWindow) -> Option<tauri::Monitor> {
    let monitors = window.available_monitors().ok()?;
    if monitors.is_empty() {
        return window.primary_monitor().ok().flatten();
    }
    monitors.into_iter().max_by_key(|m| {
        let wa = m.work_area();
        wa.position.x + wa.size.width as i32
    })
}

/// Restore saved coords, or top-right of work area. Runs in `setup` so placement applies before WebView paint.
fn apply_startup_window_position(app: &tauri::AppHandle, window: &tauri::WebviewWindow) {
    const CORNER_MARGIN_PX: i32 = 12;
    const COMPACT_LOGICAL_W: i32 = 120;

    if let Ok(path) = get_state_file(app) {
        if path.exists() {
            if let Ok(raw) = fs::read_to_string(&path) {
                if let Ok(pos) = serde_json::from_str::<WindowPosition>(&raw) {
                    let _ = window.set_position(tauri::PhysicalPosition::new(pos.x, pos.y));
                    eprintln!(
                        "[UCE] startup position from file (physical px): {}, {}",
                        pos.x, pos.y
                    );
                    return;
                }
            }
        }
    }

    let mon = monitor_for_top_right_corner(window)
        .or_else(|| window.primary_monitor().ok().flatten())
        .or_else(|| window.current_monitor().ok().flatten());

    let Some(mon) = mon else {
        eprintln!("[UCE] startup: no monitor — skipping corner placement");
        return;
    };

    let wa = mon.work_area();
    let outer = window.outer_size().unwrap_or(tauri::PhysicalSize::new(86, 44));
    let sf = mon.scale_factor();
    let mut outer_w = outer.width as i32;
    let max_outer = (140.0 * sf).round() as i32;
    if outer_w > max_outer {
        outer_w = max_outer;
    }
    if outer_w < 8 {
        outer_w = (COMPACT_LOGICAL_W as f64 * sf).round() as i32;
    }
    let x = wa.position.x + wa.size.width as i32 - outer_w - CORNER_MARGIN_PX;
    let y = wa.position.y + CORNER_MARGIN_PX;
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    eprintln!(
        "[UCE] startup position top-right (physical px): x={} y={} outer_w={} work_area origin=({}, {}) size={}x{} monitor={:?}",
        x,
        y,
        outer_w,
        wa.position.x,
        wa.position.y,
        wa.size.width,
        wa.size.height,
        mon.name()
    );
}

fn schedule_startup_position_retry(app: &tauri::AppHandle) {
    let handle = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(280));
        let handle2 = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            if let Some(w) = handle2.get_webview_window("main") {
                apply_startup_window_position(&handle2, &w);
            }
        });
    });
}

/// When the dev server is down or packaged assets are missing, WebView2 shows `chrome-error://…`
/// (“Hmmm… can't reach this page”) inside the tiny overlay — users only see a clipped “Hmm” tile.
static UCE_WEBVIEW_FINAL_ALERT_SHOWN: AtomicBool = AtomicBool::new(false);

const UCE_SAFE_OVERLAY_W: u32 = 420;
const UCE_SAFE_OVERLAY_H: u32 = 220;
const UCE_TOOLBAR_OVERLAY_W: u32 = 58;
const UCE_TOOLBAR_OVERLAY_H: u32 = 38;
const UCE_DEV_VITE_ADDR: &str = "127.0.0.1:5173";
/// Matches `tauri.conf.json` `build.devUrl` when the embedded config omits `dev_url` at runtime.
const UCE_FALLBACK_DEV_APP_URL: &str = "http://127.0.0.1:5173/";

fn uce_url_looks_like_chrome_interstitial_error(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("chrome-error:") || lower.contains("chromewebdata")
}

/// `about:blank` during WebView2 startup is normal until navigation to devUrl / asset URL completes.
fn uce_is_about_blank(url: &str) -> bool {
    matches!(
        url.trim().to_ascii_lowercase().as_str(),
        "about:blank" | "about:srcdoc"
    ) || url
        .trim()
        .to_ascii_lowercase()
        .starts_with("about:blank?")
}

/// Dev (Vite) or packaged Tauri `tauri.localhost` / asset loading — UI is considered present.
fn uce_url_looks_like_loaded_app_ui(url: &str) -> bool {
    uce_webview_url::url_looks_like_loaded_app_ui(url)
}

/// Debug builds load from Vite — verify port is listening before showing the tiny overlay.
fn uce_dev_vite_server_reachable() -> bool {
    let Ok(addr) = UCE_DEV_VITE_ADDR.parse::<SocketAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(600)).is_ok()
}

#[cfg(windows)]
fn uce_dev_server_unreachable_message_box() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;
    const MB_OK: u32 = 0;
    const MB_ICONWARNING: u32 = 0x30;
    const MB_SETFOREGROUND: u32 = 0x0001_0000;
    let title: Vec<u16> = OsStr::new("UCE — Dev server not running")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let body = "UCE could not reach the Vite dev server at http://127.0.0.1:5173.\r\n\r\n\
From the repository root run:\r\n\
  npm run tauri dev\r\n\r\n\
Do not start only the .exe while the dev server is stopped. After Vite is running, use the tray menu “Reload UCE Interface”.";
    let text: Vec<u16> = OsStr::new(body)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONWARNING | MB_SETFOREGROUND,
        );
    }
}

/// In dev, `reload()` while the document is still `about:blank` only reloads blank — it does not open Vite.
/// Use an explicit `navigate()` to `build.devUrl` (see `tauri.conf.json` `devUrl`) so recovery can succeed.
fn uce_webview_reload_or_navigate_to_configured_app(
    w: &tauri::WebviewWindow,
    app: &AppHandle,
) -> Result<(), String> {
    #[cfg(debug_assertions)]
    {
        let target = app.config().build.dev_url.clone().or_else(|| {
            eprintln!(
                "UCE_WEBVIEW_RECOVERY_NAVIGATE_FALLBACK dev_url_missing_using {}",
                UCE_FALLBACK_DEV_APP_URL
            );
            Url::parse(UCE_FALLBACK_DEV_APP_URL).ok()
        });
        if let Some(u) = target {
            eprintln!("UCE_WEBVIEW_RECOVERY_NAVIGATE {}", u);
            return w.navigate(u).map_err(|e| e.to_string());
        }
    }
    w.reload().map_err(|e| e.to_string())
}

/// Wait for `devUrl` (or packaged UI URL) to commit — navigation is asynchronous; `w.url()` can lag `NAVIGATION_STARTED`.
///
/// `run_on_main_thread` may return before the closure runs, so we **do not** read shared flags immediately.
/// Each tick: post read to main thread, then **block on a channel** until the URL snapshot is sent back.
fn uce_recovery_poll_until_loaded_app_ui(app: &AppHandle, attempt: u32) -> bool {
    const TICK_MS: u64 = 150;
    const MAX_TICKS: u32 = 45;
    const RECV_WAIT: Duration = Duration::from_secs(10);

    for tick in 0..MAX_TICKS {
        thread::sleep(Duration::from_millis(TICK_MS));
        let (tx, rx) = mpsc::channel::<(bool, String)>();
        let app_c = app.clone();
        if let Err(e) = app.run_on_main_thread(move || {
            let (ok, url) = match app_c.get_webview_window("main") {
                Some(w) => match w.url() {
                    Ok(u) => {
                        let s = u.as_str().to_string();
                        (uce_url_looks_like_loaded_app_ui(&s), s)
                    }
                    Err(_) => (false, "<url() error>".to_string()),
                },
                None => (false, "<no main window>".to_string()),
            };
            let _ = tx.send((ok, url));
        }) {
            eprintln!("[UCE] recovery poll run_on_main_thread: {e}");
            continue;
        }

        match rx.recv_timeout(RECV_WAIT) {
            Ok((true, url)) => {
                eprintln!("UCE_WEBVIEW_LOAD_CONFIRMED_BY_URL {}", url);
                eprintln!(
                    "UCE_WEBVIEW_CURRENT_URL phase=recovery_poll_loaded attempt={} ticks={}",
                    attempt,
                    tick + 1
                );
                return true;
            }
            Ok((false, url)) => {
                if tick == 0 || (tick + 1) % 10 == 0 {
                    eprintln!(
                        "UCE_WEBVIEW_CURRENT_URL phase=recovery_poll_tick attempt={} tick={} {}",
                        attempt,
                        tick + 1,
                        url
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "[UCE] recovery poll channel wait failed attempt={} tick={} err={e:?}",
                    attempt, tick
                );
            }
        }
    }
    false
}

fn uce_apply_overlay_logical_size(w: &tauri::WebviewWindow, width: u32, height: u32) -> Result<(), String> {
    use tauri::LogicalSize;
    const MAX_W: f64 = 700.0;
    const MAX_H: f64 = 2000.0;
    let wf = (width as f64).clamp(38.0, MAX_W);
    let hf = (height as f64).clamp(38.0, MAX_H);
    let _ = w.set_resizable(true);
    let _ = w.set_min_size(Some(LogicalSize::new(1.0, 1.0)));
    let _ = w.set_max_size(Some(LogicalSize::new(MAX_W, MAX_H)));
    w.set_size(tauri::Size::Logical(LogicalSize::new(wf, hf)))
        .map_err(|e| e.to_string())?;
    let _ = w.set_resizable(false);
    Ok(())
}

fn uce_bring_overlay_foreground(app: &AppHandle) -> Result<(), String> {
    let Some(w) = app.get_webview_window("main") else {
        return Err("no main window".to_string());
    };
    w.set_skip_taskbar(true).map_err(|e| e.to_string())?;
    w.show().map_err(|e| e.to_string())?;
    w.set_always_on_top(true).map_err(|e| e.to_string())?;
    w.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

fn uce_tray_tooltip(app: &AppHandle) -> String {
    match tenant_config::load_tenant_config(app) {
        Ok(cfg) => {
            let ok = !cfg.business_id.trim().is_empty()
                && !cfg.backend_url.trim().is_empty()
                && !cfg.anon_key.trim().is_empty();
            if ok {
                "UCE — Connected (FileWisely)".to_string()
            } else {
                "UCE — Not connected — use tray → Connect to FileWisely".to_string()
            }
        }
        Err(_) => "UCE — Not connected".to_string(),
    }
}

fn uce_open_connection_doctor(app: &AppHandle, view: &str) -> Result<(), String> {
    let label = "connection-doctor";
    let v = if view == "status" { "status" } else { "connect" };
    let webview_url = if cfg!(debug_assertions) {
        let path = format!("connection-doctor.html?view={v}");
        let u = Url::parse(&format!("http://127.0.0.1:5173/{path}")).map_err(|e| e.to_string())?;
        WebviewUrl::External(u)
    } else {
        let u = Url::parse(&format!(
            "http://tauri.localhost/connection-doctor.html#{v}"
        ))
        .map_err(|e| e.to_string())?;
        WebviewUrl::External(u)
    };

    if let Some(win) = app.get_webview_window(label) {
        let u = if let WebviewUrl::External(u) = &webview_url {
            u.clone()
        } else {
            return Err("connection doctor: unexpected WebviewUrl".to_string());
        };
        win.navigate(u).map_err(|e| e.to_string())?;
        win.show().map_err(|e| e.to_string())?;
        win.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    WebviewWindowBuilder::new(app, label, webview_url)
        .title("UCE — FileWisely connection")
        .inner_size(520.0, 580.0)
        .min_inner_size(420.0, 400.0)
        .resizable(true)
        .decorations(true)
        .center()
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn uce_open_connection_doctor_cmd(app: AppHandle, view: String) -> Result<(), String> {
    uce_open_connection_doctor(&app, &view)
}

#[tauri::command]
fn uce_refresh_tray_connection_tooltip(app: AppHandle) -> Result<(), String> {
    let tip = uce_tray_tooltip(&app);
    let tid = TrayIconId::new("uce-main-tray");
    if let Some(tray) = app.tray_by_id(&tid) {
        tray.set_tooltip(Some(tip)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn save_tenant_manual_all(
    app: AppHandle,
    business_id: String,
    backend_url: String,
    anon_key: String,
) -> Result<(), String> {
    tenant_config::save_tenant_manual_all(&app, business_id, backend_url, anon_key)
}

fn uce_tray_reload_interface(app: &AppHandle) -> Result<(), String> {
    eprintln!("UCE_WEBVIEW_MANUAL_RELOAD");
    UCE_WEBVIEW_FINAL_ALERT_SHOWN.store(false, Ordering::SeqCst);
    let Some(w) = app.get_webview_window("main") else {
        return Err("no main window".to_string());
    };
    uce_webview_reload_or_navigate_to_configured_app(&w, app)?;
    uce_apply_overlay_logical_size(&w, UCE_TOOLBAR_OVERLAY_W, UCE_TOOLBAR_OVERLAY_H)?;
    uce_bring_overlay_foreground(app)?;
    let h = app.clone();
    schedule_startup_position_retry(&h);
    Ok(())
}

fn uce_try_build_tray(app: &tauri::AppHandle) {
    let status_i = match MenuItem::with_id(
        app,
        "uce-tray-status",
        "Connection Status",
        true,
        None::<&str>,
    ) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[UCE] tray Connection Status: {e}");
            return;
        }
    };
    let connect_i = match MenuItem::with_id(
        app,
        "uce-tray-connect",
        "Connect to FileWisely",
        true,
        None::<&str>,
    ) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[UCE] tray Connect item: {e}");
            return;
        }
    };
    let copy_i = match MenuItem::with_id(
        app,
        "uce-tray-copy-report",
        "Copy Diagnostic Report",
        true,
        None::<&str>,
    ) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[UCE] tray Copy Report: {e}");
            return;
        }
    };
    let reload_i = match MenuItem::with_id(
        app,
        "uce-tray-reload-ui",
        "Reload UCE Interface",
        true,
        None::<&str>,
    ) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[UCE] tray menu Reload item: {e}");
            return;
        }
    };
    let quit_i = match MenuItem::with_id(
        app,
        "uce-tray-quit",
        "Quit UCE",
        true,
        None::<&str>,
    ) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[UCE] tray menu Quit item: {e}");
            return;
        }
    };
    let menu = match Menu::with_items(
        app,
        &[
            &status_i, &connect_i, &copy_i, &reload_i, &quit_i,
        ],
    ) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[UCE] tray menu: {e}");
            return;
        }
    };

    let tooltip = uce_tray_tooltip(app);
    let mut builder = TrayIconBuilder::with_id(TrayIconId::new("uce-main-tray"))
        .tooltip(&tooltip)
        .menu(&menu)
        .on_menu_event(|app, event| {
            match event.id.as_ref() {
                "uce-tray-status" => {
                    if let Err(e) = uce_open_connection_doctor(app, "status") {
                        eprintln!("[UCE] tray Connection Status: {e}");
                    }
                }
                "uce-tray-connect" => {
                    if let Err(e) = uce_open_connection_doctor(app, "connect") {
                        eprintln!("[UCE] tray Connect: {e}");
                    }
                }
                "uce-tray-copy-report" => {
                    match connection_diagnostics::uce_copy_diagnostic_report(app.clone()) {
                        Ok(_) => eprintln!("[UCE] diagnostic report copied to clipboard"),
                        Err(e) => eprintln!("[UCE] copy diagnostic report: {e}"),
                    }
                }
                "uce-tray-reload-ui" => {
                    if let Err(e) = uce_tray_reload_interface(app) {
                        eprintln!("[UCE] tray reload: {e}");
                    }
                }
                "uce-tray-quit" => {
                    app.exit(0);
                }
                _ => {}
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    if let Err(e) = builder.build(app) {
        eprintln!("[UCE] tray icon build failed: {e}");
    }
}

/// After startup, verify the webview reached the app UI. `about:blank` on pass 1 is treated as timing only.
fn schedule_uce_webview_load_failure_check(app: AppHandle) {
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(2));
        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            uce_webview_startup_health_pass(app2, 1);
        });
    });
}

/// Pass 1 @ ~2s: if UI not loaded yet (e.g. `about:blank`), wait and re-check once — no error for blank alone.
fn uce_webview_startup_health_pass(app: AppHandle, pass: u32) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let Ok(current) = w.url() else {
        return;
    };
    let url_str = current.as_str();
    eprintln!(
        "UCE_WEBVIEW_CURRENT_URL phase=startup_check pass={} {}",
        pass, url_str
    );

    if uce_url_looks_like_loaded_app_ui(url_str) {
        return;
    }

    if uce_url_looks_like_chrome_interstitial_error(url_str) {
        eprintln!("UCE_WEBVIEW_CHROME_ERROR_DETECTED {}", url_str);
        uce_run_chrome_error_recovery_or_alert(app);
        return;
    }

    if pass == 1 {
        let app_delayed = app.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(3));
            let app_d = app_delayed.clone();
            let _ = app_delayed.run_on_main_thread(move || {
                uce_webview_startup_health_pass(app_d, 2);
            });
        });
        return;
    }

    if uce_is_about_blank(url_str) {
        eprintln!("UCE_WEBVIEW_RELOAD_FAILED reason=startup_blank_stuck url={}", url_str);
    } else {
        eprintln!(
            "UCE_WEBVIEW_RELOAD_FAILED reason=dev_ui_not_reached url={}",
            url_str
        );
    }
    uce_run_chrome_error_recovery_or_alert(app);
}

/// Resize to a readable overlay, reload up to 5×, then MessageBox if the UI still will not load.
fn uce_run_chrome_error_recovery_or_alert(app: AppHandle) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let Ok(current) = w.url() else {
        return;
    };
    let url_str = current.as_str();
    eprintln!("UCE_WEBVIEW_CURRENT_URL phase=recovery_begin {}", url_str);
    if uce_url_looks_like_chrome_interstitial_error(url_str) {
        eprintln!("UCE_WEBVIEW_CHROME_ERROR_DETECTED {}", url_str);
    }

    if let Err(e) = uce_apply_overlay_logical_size(&w, UCE_SAFE_OVERLAY_W, UCE_SAFE_OVERLAY_H) {
        eprintln!("UCE_OVERLAY_SAFE_RESIZE_AFTER_LOAD_FAILURE error={}", e);
    } else {
        eprintln!("UCE_OVERLAY_SAFE_RESIZE_AFTER_LOAD_FAILURE w={} h={}", UCE_SAFE_OVERLAY_W, UCE_SAFE_OVERLAY_H);
    }

    let app_thread = app.clone();
    thread::spawn(move || {
        for attempt in 1u32..=5 {
            let app_r = app_thread.clone();
            let _ = app_thread.run_on_main_thread(move || {
                if let Some(w) = app_r.get_webview_window("main") {
                    eprintln!("UCE_WEBVIEW_RELOAD_ATTEMPT {}", attempt);
                    if let Err(e) =
                        uce_webview_reload_or_navigate_to_configured_app(&w, &app_r)
                    {
                        eprintln!("UCE_WEBVIEW_RELOAD_FAILED reload_call_err={}", e);
                    }
                }
            });

            if uce_recovery_poll_until_loaded_app_ui(&app_thread, attempt) {
                let app_v = app_thread.clone();
                let att = attempt;
                let _ = app_thread.run_on_main_thread(move || {
                    let Some(w) = app_v.get_webview_window("main") else {
                        return;
                    };
                    eprintln!("UCE_WEBVIEW_RELOAD_SUCCESS attempt={}", att);
                    if let Err(e) = uce_apply_overlay_logical_size(
                        &w,
                        UCE_TOOLBAR_OVERLAY_W,
                        UCE_TOOLBAR_OVERLAY_H,
                    ) {
                        eprintln!("[UCE] post-recovery resize: {}", e);
                    }
                    let _ = w.show();
                    let h = app_v.clone();
                    let _ = uce_bring_overlay_foreground(&app_v);
                    schedule_startup_position_retry(&h);
                });
                return;
            }

            let app_snap = app_thread.clone();
            let att = attempt;
            let _ = app_thread.run_on_main_thread(move || {
                if let Some(w) = app_snap.get_webview_window("main") {
                    if let Ok(u) = w.url() {
                        eprintln!(
                            "UCE_WEBVIEW_CURRENT_URL phase=after_reload_try attempt={} {}",
                            att,
                            u.as_str()
                        );
                        eprintln!(
                            "UCE_WEBVIEW_RELOAD_FAILED attempt={} url={} reason=poll_timeout",
                            att,
                            u.as_str()
                        );
                    }
                }
            });
        }

        let app_end = app_thread.clone();
        let _ = app_thread.run_on_main_thread(move || {
            uce_webview_finish_recovery_if_still_broken(app_end);
        });
    });
}

fn uce_webview_finish_recovery_if_still_broken(app: AppHandle) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let Ok(current) = w.url() else {
        return;
    };
    let url_str = current.as_str();
    eprintln!("UCE_WEBVIEW_CURRENT_URL phase=after_retries {}", url_str);
    if uce_url_looks_like_loaded_app_ui(url_str) {
        return;
    }
    eprintln!("UCE_WEBVIEW_RELOAD_FAILED after_all_retries url={}", url_str);

    if UCE_WEBVIEW_FINAL_ALERT_SHOWN
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    #[cfg(windows)]
    uce_webview_load_failed_native_message_box();
    #[cfg(not(windows))]
    eprintln!(
        "[UCE] UI failed to load. Dev: run `npm run tauri dev` from the repo. Packaged: reinstall UCE / FileWisely."
    );
}

/// Windows-only: full-screen dialog so the message is never clipped by the 38×38 overlay.
#[cfg(windows)]
fn uce_webview_load_failed_native_message_box() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;
    const MB_OK: u32 = 0;
    const MB_ICONWARNING: u32 = 0x30;
    const MB_SETFOREGROUND: u32 = 0x0001_0000;
    let title: Vec<u16> = OsStr::new("UCE — Could not load interface")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let body = if cfg!(debug_assertions) {
        "The UCE window could not load the app. The WebView is showing Edge’s “can’t reach this page” screen.\r\n\r\n\
If you are developing: from the repository root run:\r\n\
  npm run tauri dev\r\n\
That starts Vite on http://127.0.0.1:5173 and then launches UCE. Do not run only the .exe while the dev server is stopped.\r\n\r\n\
If you opened a packaged build: use Repair in Add/Remove Programs or reinstall the FileWisely / UCE installer."
    } else {
        "UCE could not load its user interface from disk (the embedded web assets failed to load).\r\n\r\n\
Try repairing or reinstalling UCE from your FileWisely / shop installer.\r\n\r\n\
Developers: run `npm run tauri dev` from the repo (Vite on 127.0.0.1:5173) — do not start only the .exe without the dev server."
    };
    let text: Vec<u16> = OsStr::new(body)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONWARNING | MB_SETFOREGROUND,
        );
    }
}

#[tauri::command]
fn get_watch_context(app: tauri::AppHandle) -> Result<WatchContext, String> {
    evaluate_context(&app)
}

#[tauri::command]
async fn capture_screen(app: tauri::AppHandle) -> Result<CaptureResponse, String> {
    let temp_dir = std::env::temp_dir();
    let file_path: PathBuf = temp_dir.join("uce_capture.png");
    let screens = Screen::all().map_err(|e| e.to_string())?;
    let screen = screens.get(0).ok_or("No screen found")?;
    let image = screen.capture().map_err(|e| e.to_string())?;
    image.save(&file_path).map_err(|e| e.to_string())?;

    let (source_app, window_title) = current_window_info();
    let rule_match = classify_current_context(&source_app, &window_title);
    let _ = mark_capture(&app);
    let captured_at_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let image_bytes = std::fs::read(&file_path).map_err(|e| e.to_string())?;
    let image_base64 = general_purpose::STANDARD.encode(image_bytes);

    Ok(CaptureResponse {
        success: true,
        image_base64,
        file_path: file_path.to_string_lossy().to_string(),
        message: format!(
            "Capture completed.\nApp: {}\nTitle: {}\nRule: {}",
            source_app, window_title, rule_match.rule_id
        ),
        source_app,
        window_title,
        matched_rule: rule_match.rule_id,
        changed: false,
        in_cooldown: false,
        captured_at_unix_ms,
    })
}

fn file_time_unix_ms(path: &PathBuf) -> Option<i64> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let ms = modified.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
    Some(ms)
}

/// Convert `.doc` / `.docx` in **FileWisely Incoming only**, ignoring the `since` mtime filter.
/// Batch CCC prints can produce Word files whose timestamps are slightly before UCE's JS `autoPdfSinceUnixMs`,
/// which would otherwise skip conversion in `try_convert_recent_office_docs`.
fn try_convert_incoming_word_files_now(app: &tauri::AppHandle) {
    if print_config::ccc_temp_watch_only() {
        // CCC test mode: only top-level temp → staging → convert via watcher + `ccc_batch::poll_ccc_temp_sweep`.
        return;
    }
    let cfg = pdf_watch_config::load_pdf_watch_config(app);
    let min_word_bytes: u64 = 64;
    let dir = print_config::watched_incoming_root();
    if !dir.exists() {
        return;
    }
    let entries = match fs::read_dir(&dir) {
        Ok(v) => v,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_word = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                let e = ext.to_lowercase();
                e == "doc" || e == "docx"
            })
            .unwrap_or(false);
        if !is_word {
            continue;
        }
        if fs::metadata(&path).map(|m| m.len()).unwrap_or(0) < min_word_bytes {
            continue;
        }
        let out_dir = path.parent().map(PathBuf::from).unwrap_or_else(|| dir.clone());
        let stem = path.file_stem().and_then(|s| s.to_str());
        let Some(stem) = stem else {
            continue;
        };
        let pdf_path = out_dir.join(format!("{stem}.pdf"));
        if !services::converter::needs_conversion(&path, &pdf_path) {
            continue;
        }

        match services::office_printer_route::route_office_document_printer_first(
            app, &path, &cfg, "poll_incoming",
        ) {
            services::office_printer_route::OfficePrinterFirstResult::SkipDuplicateWindow => {
                continue;
            }
            services::office_printer_route::OfficePrinterFirstResult::HandledByPrinter => {
                continue;
            }
            services::office_printer_route::OfficePrinterFirstResult::FallThroughToStaging => {}
        }

        if !cfg.word_to_pdf_enabled {
            continue;
        }
        let Some(soffice) =
            services::converter::resolve_soffice_path(cfg.libreoffice_path.as_deref())
        else {
            continue;
        };

        let t0 = Instant::now();
        let path_display = path.display().to_string();
        match services::converter::ingest_office_incoming_to_pdf(
            &soffice,
            &path,
            &out_dir,
            stem,
            Some(t0),
            "poll_incoming",
        ) {
            Ok(pdf) => {
                eprintln!(
                    "[UCE] OFFICE_ENQUEUED_AS_PDF path={}",
                    pdf.display()
                );
                eprintln!(
                    "[UCE] OFFICE_INGESTION_MODE=staging_convert success pdf={}",
                    pdf.display()
                );
                if cfg.delete_word_after_convert {
                    let _ = fs::remove_file(&path);
                }
                services::incoming_emit::emit_uce_incoming_pdf(
                    app,
                    pdf.to_string_lossy().to_string(),
                );
            }
            Err(e)
                if e == services::converter::DUPLICATE_OFFICE_PIPELINE_SKIPPED =>
            {
                eprintln!(
                    "[UCE] OFFICE_PIPELINE_RESULT path={} result=skipped reason=duplicate_pipeline",
                    path_display
                );
            }
            Err(e) => {
                eprintln!(
                    "[UCE] OFFICE_FINAL_ERROR path={} err={}",
                    path_display, e
                );
                eprintln!(
                    "[UCE] OFFICE_PIPELINE_RESULT path={} result=failed",
                    path_display
                );
            }
        }
    }
}

/// Convert recent `.doc` / `.docx` in the same folders as PDF watch, so the normal PDF pipeline can pick up the PDF.
fn try_convert_recent_office_docs(app: &tauri::AppHandle, start_unix_ms: i64) {
    if print_config::ccc_temp_watch_only() {
        // Avoid scanning Downloads / FileWisely / `.uce_staging` with wrong stems; CCC uses poll helper only.
        return;
    }
    let cfg = pdf_watch_config::load_pdf_watch_config(app);
    let min_word_bytes: u64 = 64;

    for dir in pdf_watch_config::candidate_pdf_dirs(app) {
        if !dir.exists() {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_word = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| {
                    let e = ext.to_lowercase();
                    e == "doc" || e == "docx"
                })
                .unwrap_or(false);
            if !is_word {
                continue;
            }
            let Some(ts) = file_time_unix_ms(&path) else {
                continue;
            };
            if ts < start_unix_ms {
                continue;
            }
            if fs::metadata(&path).map(|m| m.len()).unwrap_or(0) < min_word_bytes {
                continue;
            }
            let (out_dir, stem) = services::converter::office_output_dir_and_pdf_stem(&path);
            let pdf_path = out_dir.join(format!("{stem}.pdf"));
            if !services::converter::needs_conversion(&path, &pdf_path) {
                continue;
            }

            match services::office_printer_route::route_office_document_printer_first(
                app, &path, &cfg, "poll_recent",
            ) {
                services::office_printer_route::OfficePrinterFirstResult::SkipDuplicateWindow => {
                    continue;
                }
                services::office_printer_route::OfficePrinterFirstResult::HandledByPrinter => {
                    continue;
                }
                services::office_printer_route::OfficePrinterFirstResult::FallThroughToStaging => {}
            }

            if !cfg.word_to_pdf_enabled {
                continue;
            }
            let Some(soffice) =
                services::converter::resolve_soffice_path(cfg.libreoffice_path.as_deref())
            else {
                continue;
            };

            let t0 = Instant::now();
            let path_display = path.display().to_string();
            match services::converter::ingest_office_incoming_to_pdf(
                &soffice,
                &path,
                &out_dir,
                stem.as_str(),
                Some(t0),
                "poll_recent",
            ) {
                Ok(pdf) => {
                    eprintln!(
                        "[UCE] OFFICE_ENQUEUED_AS_PDF path={}",
                        pdf.display()
                    );
                    eprintln!(
                        "[UCE] OFFICE_INGESTION_MODE=staging_convert success pdf={}",
                        pdf.display()
                    );
                    if cfg.delete_word_after_convert {
                        let _ = fs::remove_file(&path);
                    }
                    services::incoming_emit::emit_uce_incoming_pdf(
                        app,
                        pdf.to_string_lossy().to_string(),
                    );
                }
                Err(e)
                    if e == services::converter::DUPLICATE_OFFICE_PIPELINE_SKIPPED =>
                {
                    eprintln!(
                        "[UCE] OFFICE_PIPELINE_RESULT path={} result=skipped reason=duplicate_pipeline",
                        path_display
                    );
                }
                Err(e) => {
                    eprintln!(
                        "[UCE] OFFICE_FINAL_ERROR path={} err={}",
                        path_display, e
                    );
                    eprintln!(
                        "[UCE] OFFICE_PIPELINE_RESULT path={} result=failed",
                        path_display
                    );
                }
            }
        }
    }
}

fn newest_pdf_since(app: &tauri::AppHandle, start_unix_ms: i64) -> Option<PathBuf> {
    services::ccc_batch::poll_ccc_temp_sweep(app);
    try_convert_incoming_word_files_now(app);
    try_convert_recent_office_docs(app, start_unix_ms);
    let cfg = pdf_watch_config::load_pdf_watch_config(app);
    let min_b = pdf_watch_config::min_pdf_bytes(&cfg);
    let mut best: Option<(i64, PathBuf)> = None;
    for dir in pdf_watch_config::candidate_pdf_dirs(app) {
        if !dir.exists() {
            continue;
        }
        let entries = match fs::read_dir(dir) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_pdf = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("pdf"))
                .unwrap_or(false);
            if !is_pdf {
                continue;
            }
            let ts = match file_time_unix_ms(&path) {
                Some(v) if v >= start_unix_ms => v,
                _ => continue,
            };
            let size_ok = fs::metadata(&path).map(|m| m.len() > min_b).unwrap_or(false);
            if !size_ok {
                continue;
            }
            match &best {
                Some((best_ts, _)) if ts <= *best_ts => {}
                _ => best = Some((ts, path)),
            }
        }
    }
    best.map(|(_, p)| p)
}

fn newest_pdf_meta_since(app: &tauri::AppHandle, start_unix_ms: i64) -> Option<PdfMetaResponse> {
    services::ccc_batch::poll_ccc_temp_sweep(app);
    try_convert_incoming_word_files_now(app);
    try_convert_recent_office_docs(app, start_unix_ms);
    let cfg = pdf_watch_config::load_pdf_watch_config(app);
    let min_b = pdf_watch_config::min_pdf_bytes(&cfg);
    let mut best: Option<(i64, PathBuf, u64)> = None;
    for dir in pdf_watch_config::candidate_pdf_dirs(app) {
        if !dir.exists() {
            continue;
        }
        let entries = match fs::read_dir(dir) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_pdf = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("pdf"))
                .unwrap_or(false);
            if !is_pdf {
                continue;
            }
            let ts = match file_time_unix_ms(&path) {
                Some(v) if v >= start_unix_ms => v,
                _ => continue,
            };
            let file_size = match fs::metadata(&path) {
                Ok(m) if m.len() > min_b => m.len(),
                _ => continue,
            };
            match &best {
                Some((best_ts, _, _)) if ts <= *best_ts => {}
                _ => best = Some((ts, path, file_size)),
            }
        }
    }
    best.map(|(modified_unix_ms, p, file_size)| PdfMetaResponse {
        file_path: p.to_string_lossy().to_string(),
        modified_unix_ms,
        file_size,
    })
}

/// All PDFs in watched folders with mtime ≥ `since_unix_ms`, sorted by (mtime, path).
/// Used to upload multi-print batches (CCC “print N documents”) — not only the single newest file.
fn collect_pdf_metas_since(app: &tauri::AppHandle, since_unix_ms: i64) -> Vec<PdfMetaResponse> {
    services::ccc_batch::poll_ccc_temp_sweep(app);
    try_convert_incoming_word_files_now(app);
    try_convert_recent_office_docs(app, since_unix_ms);
    let cfg = pdf_watch_config::load_pdf_watch_config(app);
    let min_b = pdf_watch_config::min_pdf_bytes(&cfg);
    let mut out: Vec<PdfMetaResponse> = Vec::new();
    for dir in pdf_watch_config::candidate_pdf_dirs(app) {
        if !dir.exists() {
            continue;
        }
        let entries = match fs::read_dir(dir) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_pdf = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("pdf"))
                .unwrap_or(false);
            if !is_pdf {
                continue;
            }
            let ts = match file_time_unix_ms(&path) {
                Some(v) if v >= since_unix_ms => v,
                _ => continue,
            };
            let file_size = match fs::metadata(&path) {
                Ok(m) if m.len() > min_b => m.len(),
                _ => continue,
            };
            out.push(PdfMetaResponse {
                file_path: path.to_string_lossy().to_string(),
                modified_unix_ms: ts,
                file_size,
            });
        }
    }
    out.sort_by(|a, b| {
        a.modified_unix_ms
            .cmp(&b.modified_unix_ms)
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    out
}

#[tauri::command]
fn list_pdf_metas_since(app: tauri::AppHandle, since_unix_ms: i64) -> Vec<PdfMetaResponse> {
    collect_pdf_metas_since(&app, since_unix_ms)
}

/// All `fw_*.pdf` in FileWisely Incoming (upload rescue / trace). Does not run Word→PDF conversion.
fn collect_fw_pdf_metas_in_filewisely_incoming(app: &tauri::AppHandle) -> Vec<PdfMetaResponse> {
    let dir = print_config::watched_incoming_root();
    let cfg = pdf_watch_config::load_pdf_watch_config(app);
    let min_b = pdf_watch_config::min_pdf_bytes(&cfg);
    let mut out: Vec<PdfMetaResponse> = Vec::new();
    if !dir.exists() {
        return out;
    }
    let entries = match fs::read_dir(&dir) {
        Ok(v) => v,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let n = name.to_lowercase();
        if !n.starts_with("fw_") || !n.ends_with(".pdf") {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let ts = match file_time_unix_ms(&path) {
            Some(v) => v,
            None => continue,
        };
        let file_size = match fs::metadata(&path) {
            Ok(m) if m.len() > min_b => m.len(),
            _ => continue,
        };
        out.push(PdfMetaResponse {
            file_path: path.to_string_lossy().to_string(),
            modified_unix_ms: ts,
            file_size,
        });
    }
    out.sort_by(|a, b| {
        a.modified_unix_ms
            .cmp(&b.modified_unix_ms)
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    out
}

#[tauri::command]
fn list_fw_pdf_metas_in_filewisely_incoming(app: tauri::AppHandle) -> Vec<PdfMetaResponse> {
    collect_fw_pdf_metas_in_filewisely_incoming(&app)
}

#[tauri::command]
async fn wait_for_recent_pdf(
    app: tauri::AppHandle,
    timeout_secs: Option<u64>,
) -> Result<PdfCaptureResponse, String> {
    let timeout = timeout_secs.unwrap_or(30).clamp(10, 60);
    let start_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let deadline_unix_ms = start_unix_ms + (timeout as i64 * 1000);

    loop {
        if let Some(path) = newest_pdf_since(&app, start_unix_ms) {
            let bytes = fs::read(&path).map_err(|e| e.to_string())?;
            let pdf_base64 = general_purpose::STANDARD.encode(bytes);
            let captured_at_unix_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            return Ok(PdfCaptureResponse {
                success: true,
                image_base64: pdf_base64,
                file_path: path.to_string_lossy().to_string(),
                message: "PDF captured from recent export.".to_string(),
                captured_at_unix_ms,
            });
        }

        let now_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        if now_unix_ms >= deadline_unix_ms {
            return Err(format!("No PDF found within {}s", timeout));
        }
        sleep(Duration::from_millis(500)).await;
    }
}

#[tauri::command]
fn get_newest_pdf_meta(app: tauri::AppHandle, since_unix_ms: i64) -> Option<PdfMetaResponse> {
    newest_pdf_meta_since(&app, since_unix_ms)
}

#[tauri::command]
fn get_pdf_watch_config(app: tauri::AppHandle) -> Result<PdfWatchConfig, String> {
    Ok(pdf_watch_config::load_pdf_watch_config(&app))
}

#[tauri::command]
fn save_pdf_watch_config(app: tauri::AppHandle, config: PdfWatchConfig) -> Result<(), String> {
    pdf_watch_config::save_pdf_watch_config(&app, &config)
}

/// JS auto-PDF upload: stderr lifecycle line when `UCE_CCC_TEMP_WATCH_ONLY` is set.
#[tauri::command]
fn uce_log_pdf_lifecycle(phase: String, path: String, success: Option<bool>) {
    if !print_config::ccc_temp_watch_only() {
        return;
    }
    let status = match success {
        Some(true) => "result=ok",
        Some(false) => "result=err",
        None => "",
    };
    if status.is_empty() {
        eprintln!("[UCE][ccc] lifecycle phase={phase} path={path}");
    } else {
        eprintln!("[UCE][ccc] lifecycle phase={phase} path={path} {status}");
    }
    if phase == "upload_finished" {
        services::ccc_batch::note_upload_for_batch(&path, success.unwrap_or(false));
    }
}

#[tauri::command]
fn read_pdf_file(path: String) -> Result<PdfCaptureResponse, String> {
    let file_path = PathBuf::from(path);
    if !file_path.exists() {
        return Err("PDF file not found".to_string());
    }
    let is_pdf = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false);
    if !is_pdf {
        return Err("File is not a PDF".to_string());
    }
    let bytes = fs::read(&file_path).map_err(|e| e.to_string())?;
    let pdf_base64 = general_purpose::STANDARD.encode(bytes);
    let captured_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let out_path = file_path.to_string_lossy().to_string();
    eprintln!("[UCE][pipeline] READ_PDF_OK path={}", out_path);
    Ok(PdfCaptureResponse {
        success: true,
        image_base64: pdf_base64,
        file_path: out_path,
        message: "PDF captured from background watcher.".to_string(),
        captured_at_unix_ms,
    })
}

fn path_is_under_filewisely_incoming(candidate: &Path) -> bool {
    let incoming = Path::new(print_config::FW_OUTPUT_DIR);
    let Ok(inc) = fs::canonicalize(incoming) else {
        return false;
    };
    let Ok(c) = fs::canonicalize(candidate) else {
        return false;
    };
    c.starts_with(&inc)
}

fn rename_or_copy_delete(src: &Path, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(e1) => {
            fs::copy(src, dest).map_err(|e| format!("copy failed: {e}; rename was: {e1}"))?;
            fs::remove_file(src).map_err(|e| e.to_string())?;
            Ok(())
        }
    }
}

/// Move `fw_*.pdf` from `C:\FileWisely\Incoming` to Processed or Failed; optional `{stem}.error.json` in Failed.
#[tauri::command]
fn uce_move_fw_pdf_outcome(
    source_path: String,
    outcome: String,
    error_json: Option<String>,
) -> Result<String, String> {
    let src = Path::new(&source_path);
    if !src.is_file() {
        return Err(format!("source not found or not a file: {source_path}"));
    }
    let name = src
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "invalid file name".to_string())?;
    let lower = name.to_lowercase();
    if !lower.starts_with("fw_") || !lower.ends_with(".pdf") {
        return Err("uce_move_fw_pdf_outcome: only fw_*.pdf under Incoming".to_string());
    }
    if !path_is_under_filewisely_incoming(src) {
        return Err(format!(
            "path must be under {}: {}",
            print_config::FW_OUTPUT_DIR,
            source_path
        ));
    }
    let dest_dir = match outcome.as_str() {
        "processed" => print_config::filewisely_processed_dir(),
        "failed" => print_config::filewisely_failed_dir(),
        o => return Err(format!("outcome must be processed or failed, got {o}")),
    };
    fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let mut dest = dest_dir.join(name);
    if dest.exists() {
        let stem = src
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("fw");
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        dest = dest_dir.join(format!("{stem}_{ts}.pdf"));
    }
    rename_or_copy_delete(src, &dest).map_err(|e| e.to_string())?;
    let dest_str = dest.to_string_lossy().to_string();
    if outcome == "failed" {
        let stem = dest
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let jpath = dest_dir.join(format!("{stem}.error.json"));
        let body = error_json.unwrap_or_else(|| r#"{"error":"unknown"}"#.to_string());
        fs::write(&jpath, body).map_err(|e| e.to_string())?;
        eprintln!(
            "[UCE][pipeline] FILE_MOVED_TO_FAILED path={} sidecar={}",
            dest.display(),
            jpath.display()
        );
    } else {
        eprintln!(
            "[UCE][pipeline] FILE_MOVED_TO_PROCESSED path={}",
            dest.display()
        );
    }
    Ok(dest_str)
}

/// Stderr + DevTools: one line per pipeline stage for `fw_*.pdf` after rename.
#[tauri::command]
fn uce_fw_pipeline_log(stage: String, path: String, detail: Option<String>) {
    if let Some(ref d) = detail {
        let max = 8000usize;
        if d.len() > max {
            eprintln!(
                "[UCE][pipeline] {} path={} detail_len={} detail_prefix={}...",
                stage,
                path,
                d.len(),
                &d[..max]
            );
        } else {
            eprintln!("[UCE][pipeline] {} path={} detail={}", stage, path, d);
        }
    } else {
        eprintln!("[UCE][pipeline] {} path={}", stage, path);
    }
}

#[tauri::command]
fn start_window_drag(window: tauri::Window) -> Result<(), String> {
    window.start_dragging().map_err(|e| e.to_string())
}

#[tauri::command]
fn focus_overlay(window: tauri::Window) -> Result<(), String> {
    /* Keep UCE off the taskbar / “tray row” when surfacing the overlay (shops close the app if they see it). */
    window.set_skip_taskbar(true).map_err(|e| e.to_string())?;
    window.show().map_err(|e| e.to_string())?;
    window.set_always_on_top(true).map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())
}

/// Resize overlay using native APIs (WebView2 + `setSize` from JS is unreliable for height).
/// Windows-only: real `MessageBox` so the alert is never clipped by the tiny overlay WebView (38×38).
#[cfg(windows)]
fn printer_severe_native_message_box() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;
    const MB_OK: u32 = 0;
    const MB_ICONWARNING: u32 = 0x30;
    const MB_SETFOREGROUND: u32 = 0x0001_0000;
    let title: Vec<u16> = OsStr::new("UCE — Printer issue")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let body = "FileWisely Printer was not detected (exact name match).\r\n\r\n\
UCE may try to repair automatically. If this keeps appearing, run the FileWisely installer \
or reinstall the PDF printer.\r\n\r\n\
Click OK to dismiss.";
    let text: Vec<u16> = OsStr::new(body)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONWARNING | MB_SETFOREGROUND,
        );
    }
}

#[tauri::command]
async fn uce_printer_severe_native_alert() -> Result<(), String> {
    #[cfg(windows)]
    {
        tokio::task::spawn_blocking(|| {
            printer_severe_native_message_box();
        })
        .await
        .map_err(|e| format!("printer alert: {e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err("native printer alert only on Windows".to_string())
    }
}

#[tauri::command]
fn uce_set_overlay_logical_size(
    window: tauri::WebviewWindow,
    width: u32,
    height: u32,
) -> Result<(), String> {
    uce_apply_overlay_logical_size(&window, width, height)
}

#[tauri::command]
fn save_window_position(app: tauri::AppHandle, x: i32, y: i32) -> Result<(), String> {
    let path = get_state_file(&app)?;
    let payload = serde_json::to_string(&WindowPosition { x, y }).map_err(|e| e.to_string())?;
    std::fs::write(path, payload).map_err(|e| e.to_string())
}

#[tauri::command]
fn load_window_position(app: tauri::AppHandle) -> Result<Option<WindowPosition>, String> {
    let path = get_state_file(&app)?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let pos: WindowPosition = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    Ok(Some(pos))
}

#[tauri::command]
fn get_debug_state(app: tauri::AppHandle) -> Result<DebugState, String> {
    Ok(DebugState {
        context: evaluate_context(&app)?,
        memory: load_memory(&app)?,
        known_rules: all_built_in_rules(),
        candidate_patterns: candidate_patterns(),
    })
}

#[tauri::command]
fn get_last_observed_context(app: tauri::AppHandle) -> Result<LastObservedContext, String> {
    let memory = load_memory(&app)?;
    let matched_rule = if memory.last_matched_rule.is_empty() {
        "none".to_string()
    } else {
        memory.last_matched_rule.clone()
    };
    let workflow_kind = workflow_kind_from_rule_id(&matched_rule);
    let preferred_capture_mode = preferred_capture_mode_for_rule(&matched_rule);
    Ok(LastObservedContext {
        source_app: if memory.last_context_key.is_empty() {
            "unknown".to_string()
        } else {
            memory
                .last_context_key
                .split('|')
                .next()
                .unwrap_or("unknown")
                .to_string()
        },
        window_title: if memory.last_window_title.is_empty() {
            "unknown".to_string()
        } else {
            memory.last_window_title
        },
        matched_rule,
        workflow_kind,
        preferred_capture_mode,
        bucket: if memory.last_bucket.is_empty() {
            "unknown".to_string()
        } else {
            memory.last_bucket
        },
        in_cooldown: memory.last_in_cooldown,
    })
}

#[tauri::command]
fn train_ccc_context(app: tauri::AppHandle) -> Result<String, String> {
    train_current_context_as_ccc(&app)
}

#[tauri::command]
fn train_workflow_context(app: tauri::AppHandle, workflow: String) -> Result<String, String> {
    train_current_context_for_workflow(&app, workflow)
}

#[tauri::command]
fn forget_ccc_training_for_current_context(app: tauri::AppHandle) -> Result<String, String> {
    forget_trained_rules_for_current_context(&app)
}

#[tauri::command]
fn exclude_ccc_context_for_current_window(app: tauri::AppHandle) -> Result<String, String> {
    exclude_current_context_from_ccc(&app)
}

#[tauri::command]
fn clear_ccc_excludes_for_current_window(app: tauri::AppHandle) -> Result<String, String> {
    clear_exclude_rules_for_current_context(&app)
}

/// HTTPS GET JSON `{ version?, trained, excluded, pdf_watch_extra_dirs?, pdf_watch_office_intercept_extra_dirs? }` — replaces local watch lists (and optional PDF watch fields when present).
#[tauri::command]
async fn sync_watch_policy_from_remote(
    app: tauri::AppHandle,
    url: String,
    business_id: String,
    authorization: Option<String>,
) -> Result<String, String> {
    watch_policy_sync::fetch_and_apply_watch_policy(
        &app,
        &url,
        &business_id,
        authorization.as_deref(),
    )
    .await
}

/// Apply policy when the frontend fetches JSON (e.g. CORS) and passes it in-process.
#[tauri::command]
fn apply_watch_policy_json(app: tauri::AppHandle, policy: WatchPolicyDocument) -> Result<String, String> {
    watch_policy_sync::apply_watch_policy_to_disk(&app, &policy)?;
    Ok("Watch policy applied".to_string())
}

#[tauri::command]
fn load_tenant_business_id(app: tauri::AppHandle) -> Result<Option<String>, String> {
    tenant_config::load_tenant_business_id(&app)
}

#[tauri::command]
fn save_tenant_business_id(app: tauri::AppHandle, business_id: String) -> Result<(), String> {
    tenant_config::save_tenant_business_id(&app, business_id)
}

#[tauri::command]
fn load_tenant_config(app: tauri::AppHandle) -> Result<tenant_config::TenantConfig, String> {
    tenant_config::load_tenant_config(&app)
}

#[tauri::command]
fn save_tenant_from_connect(
    app: tauri::AppHandle,
    business_id: String,
    backend_url: Option<String>,
    anon_key: Option<String>,
) -> Result<(), String> {
    tenant_config::save_tenant_from_connect(&app, business_id, backend_url, anon_key)
}

/// Copy a saved file into `C:\FileWisely\Incoming` (e.g. staff used “Save as” outside the watch folder).
#[tauri::command]
fn uce_copy_into_incoming(path: String) -> Result<String, String> {
    let p = PathBuf::from(path.trim());
    services::file_normalizer::normalize_into_incoming(&p)
        .map(|pb| pb.to_string_lossy().into_owned())
}

/// Open HTTPS in the default browser (e.g. FileWisely web app).
#[tauri::command]
fn uce_open_url(url: String) -> Result<(), String> {
    let u = url.trim();
    if !u.starts_with("https://") && !u.starts_with("http://") {
        return Err("Only http(s) URLs are allowed".to_string());
    }
    open::that(u).map_err(|e| e.to_string())
}

/// Self-monitoring: verify **FileWisely Printer** (or related PDF printers) exists.
#[tauri::command]
fn uce_check_filewisely_printer() -> Result<PrinterCheckResult, String> {
    services::printer_check::check_filewisely_printer()
}

/// Self-healing: re-run PDF printer silent install + rename (cooldown enforced in JS).
/// Bundled `pdf-printer` from the MSI (`resources` in `tauri.conf.json`) is used when `C:\FileWisely\pdf-printer` is empty.
#[tauri::command]
fn repair_printer(app: tauri::AppHandle) -> Result<RepairPrinterResult, String> {
    let mut roots: Vec<PathBuf> = vec![PathBuf::from(r"C:\FileWisely\pdf-printer")];
    if let Ok(rd) = app.path().resource_dir() {
        roots.push(rd.join("pdf-printer"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.join("resources").join("pdf-printer"));
            roots.push(dir.join("pdf-printer"));
        }
    }
    services::printer_check::repair_filewisely_printer(roots)
}

/// User or toast one-click: print an Office file to **FileWisely Printer** (Word COM, headless).
#[tauri::command]
fn uce_office_print_to_filewisely(path: String) -> Result<String, String> {
    let p = PathBuf::from(path.trim());
    if !p.is_file() {
        return Err(format!(
            "uce_office_print_to_filewisely: not a file: {}",
            p.display()
        ));
    }
    services::office_printer_route::try_print_office_to_filewisely(&p)?;
    services::office_printer_route::mark_office_printer_routed(&p);
    eprintln!(
        "[UCE] OFFICE_ROUTING_PRINT_FINISHED path={} source=manual_invoke",
        p.display()
    );
    eprintln!(
        "[UCE] OFFICE_INGESTION_MODE=printer_preferred success path={} source=manual_invoke",
        p.display()
    );
    Ok("ok".to_string())
}

/// Manual trigger (same as **Ctrl+Shift+W**): close foreground Word when a Change Request flow was recent.
#[tauri::command]
fn uce_ccc_cr_manual_close_word() -> Result<String, String> {
    services::ccc_cr_word_autoclose::manual_close_armed_word()
}

/// POST to FileWisely `uce-ro-status` from the Rust side so the WebView is not subject to browser CORS.
#[tauri::command]
async fn uce_post_ro_status(
    url: String,
    business_id: String,
    repair_order_number: String,
    window_title: String,
    device_id: String,
    authorization: String,
) -> Result<String, String> {
    let u = url.trim();
    if !u.starts_with("https://") {
        return Err("uce-ro-status URL must use HTTPS".to_string());
    }
    let auth = authorization.trim();
    if auth.is_empty() {
        return Err(
            "Missing Supabase anon key (set VITE_SUPABASE_ANON_KEY or VITE_UCE_SUPABASE_ANON_KEY)"
                .to_string(),
        );
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let ro = repair_order_number.trim().to_string();
    let body = json!({
        "business_id": business_id.trim(),
        "repair_order_number": ro,
        "ro_number": repair_order_number.trim(),
        "window_title": window_title,
        "device_id": device_id,
    });

    let resp = client
        .post(u)
        .header("Authorization", format!("Bearer {}", auth))
        .header("apikey", auth)
        .header("Content-Type", "application/json")
        .header("Cache-Control", "no-cache")
        .header("Pragma", "no-cache")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("RO status request failed: {}", e))?;

    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("RO status HTTP {}: {}", status, text));
    }
    Ok(text)
}

#[tauri::command]
fn uce_machine_name() -> String {
    sysinfo::System::host_name().unwrap_or_else(|| "unknown".to_string())
}

#[tauri::command]
fn uce_os_info() -> String {
    sysinfo::System::long_os_version().unwrap_or_else(|| "unknown".to_string())
}

#[tauri::command]
fn uce_ensure_startup_shortcut() -> Result<String, String> {
    services::startup_shortcut::ensure_filewisely_uce_shortcut()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let global_shortcut_plugin = tauri_plugin_global_shortcut::Builder::new()
        .with_shortcuts(["ctrl+shift+w", "ctrl+shift+u", "ctrl+shift+q"])
        .expect("UCE: register ctrl+shift+w/u/q (Word close / show dock / exit)")
        .with_handler(|app, shortcut, event| {
            use tauri_plugin_global_shortcut::ShortcutState;
            if event.state != ShortcutState::Pressed {
                return;
            }
            let key = format!("{shortcut}");
            let lower = key.to_lowercase();
            if lower.contains("shift+q") {
                eprintln!("[UCE] ctrl+shift+q — exiting desktop agent");
                app.exit(0);
                return;
            }
            if lower.contains("shift+u") {
                if let Err(e) = app.emit("uce-show-dock", ()) {
                    eprintln!("[UCE] emit uce-show-dock: {e}");
                }
                return;
            }
            if lower.contains("shift+w") {
                match services::ccc_cr_word_autoclose::manual_close_armed_word() {
                    Ok(m) => eprintln!("[UCE][ccc-cr] hotkey_ok {}", m),
                    Err(e) => eprintln!("[UCE][ccc-cr] hotkey_err {}", e),
                }
            }
        })
        .build();

    let mut builder = tauri::Builder::default();

    #[cfg(windows)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            eprintln!("[UCE] single-instance argv: {:?}", argv);
            // When UCE is already running, Windows may start a second process with `uce://…` on argv.
            // Forward those URLs to the webview so `tryApplyBusinessIdFromUrls` runs (browser Connect flow).
            let deeplinks: Vec<String> = argv
                .into_iter()
                .filter(|a| {
                    let s = a.to_lowercase();
                    s.contains("uce:") || s.contains("uce%3a") || s.contains("uce%3a%2f%2f")
                })
                .collect();
            if !deeplinks.is_empty() {
                eprintln!("[UCE] single-instance forwarding deeplinks to webview: {:?}", deeplinks);
                if let Err(e) = app.emit("uce-argv-deeplinks", deeplinks) {
                    eprintln!("[UCE] emit uce-argv-deeplinks: {e}");
                }
            }
        }));
    }

    builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(global_shortcut_plugin)
        .plugin(tauri_plugin_deep_link::init())
        .on_page_load(|_webview, payload| {
            match payload.event() {
                PageLoadEvent::Started => {
                    eprintln!("UCE_WEBVIEW_NAVIGATION_STARTED {}", payload.url());
                }
                PageLoadEvent::Finished => {
                    let u = payload.url().as_str();
                    eprintln!("UCE_WEBVIEW_NAVIGATION_FINISHED {}", u);
                    eprintln!("UCE_WEBVIEW_CURRENT_URL {}", u);
                    if uce_url_looks_like_chrome_interstitial_error(u) {
                        eprintln!("UCE_WEBVIEW_CHROME_ERROR_DETECTED {}", u);
                    }
                }
            }
        })
        .setup(|app| {
            #[cfg(windows)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                if let Err(e) = app.deep_link().register_all() {
                    eprintln!("[UCE] deep_link register_all: {}", e);
                }
            }
            uce_try_build_tray(app.handle());

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_skip_taskbar(true);
                let _ = window.set_resizable(false);
                let h = app.handle().clone();

                #[cfg(debug_assertions)]
                let dev_server_ok = {
                    eprintln!(
                        "UCE_WEBVIEW_CURRENT_URL phase=dev_listen target=http://{}",
                        UCE_DEV_VITE_ADDR
                    );
                    let ok = uce_dev_vite_server_reachable();
                    eprintln!("UCE_WEBVIEW_CURRENT_URL phase=dev_listen_ok value={}", ok);
                    ok
                };
                #[cfg(not(debug_assertions))]
                let dev_server_ok = true;

                if cfg!(debug_assertions) && !dev_server_ok {
                    let _ = window.hide();
                    #[cfg(windows)]
                    uce_dev_server_unreachable_message_box();
                    #[cfg(not(windows))]
                    eprintln!(
                        "[UCE] dev server not reachable at {}; run npm run tauri dev",
                        UCE_DEV_VITE_ADDR
                    );
                } else {
                    let _ = window.show();
                    apply_startup_window_position(&h, &window);
                    schedule_startup_position_retry(&h);
                }

                // Alt+F4 / close: keep agent running (heartbeats, watchers). Use Ctrl+Shift+Q to exit.
                let app_handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if let Err(e) = app_handle.emit("uce-close-keep-running", ()) {
                            eprintln!("[UCE] emit uce-close-keep-running: {e}");
                        }
                        if let Some(w) = app_handle.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                });

                if dev_server_ok {
                    schedule_uce_webview_load_failure_check(h.clone());
                }
            }
            #[cfg(windows)]
            {
                let h = app.handle().clone();
                services::print_watcher::start_print_watcher(h.clone());
                services::office_intercept::spawn_office_winword_telemetry(h);
                services::flight_recorder::spawn();
                services::processed_retention::spawn();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_watch_context,
            capture_screen,
            wait_for_recent_pdf,
            get_newest_pdf_meta,
            list_pdf_metas_since,
            list_fw_pdf_metas_in_filewisely_incoming,
            get_pdf_watch_config,
            save_pdf_watch_config,
            read_pdf_file,
            uce_move_fw_pdf_outcome,
            uce_fw_pipeline_log,
            uce_log_pdf_lifecycle,
            start_window_drag,
            focus_overlay,
            save_window_position,
            load_window_position,
            get_debug_state,
            get_last_observed_context,
            train_ccc_context,
            train_workflow_context,
            forget_ccc_training_for_current_context,
            exclude_ccc_context_for_current_window,
            clear_ccc_excludes_for_current_window,
            sync_watch_policy_from_remote,
            apply_watch_policy_json,
            load_tenant_business_id,
            load_tenant_config,
            save_tenant_business_id,
            save_tenant_from_connect,
            uce_set_overlay_logical_size,
            uce_open_url,
            uce_copy_into_incoming,
            uce_check_filewisely_printer,
            uce_printer_severe_native_alert,
            repair_printer,
            uce_office_print_to_filewisely,
            uce_post_ro_status,
            uce_ccc_cr_manual_close_word,
            uce_machine_name,
            uce_os_info,
            uce_ensure_startup_shortcut,
            save_tenant_manual_all,
            uce_open_connection_doctor_cmd,
            uce_refresh_tray_connection_tooltip,
            connection_diagnostics::uce_record_heartbeat_outcome,
            connection_diagnostics::uce_get_connection_diagnostics,
            connection_diagnostics::uce_test_ingest_connection,
            connection_diagnostics::uce_copy_diagnostic_report
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Resumed = event {
                if let Err(e) = app_handle.emit("uce-system-resumed", ()) {
                    eprintln!("[UCE] emit uce-system-resumed: {e}");
                }
            }
        });
}

fn main() {
    run();
}