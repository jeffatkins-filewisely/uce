//! Self-healing supervisor: stale heartbeat nudge, webview chrome-error recovery, health log.

use crate::ccc_package_sync;
use crate::connection_diagnostics;
use crate::device_health;
use crate::tenant_config;
use crate::{uce_run_chrome_error_recovery_or_alert, uce_url_looks_like_chrome_interstitial_error};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

const LOG_PATH: &str = r"C:\FileWisely\logs\uce-health.log";
const CHECK_INTERVAL_SECS: u64 = 60;
const HEARTBEAT_NUDGE_MS: i64 = 10 * 60 * 1000;
const CCC_ACTIVITY_STALE_MS: i64 = 20 * 60 * 1000;
const WEBVIEW_RECOVERY_COOLDOWN_MS: i64 = 3 * 60 * 1000;
const LOG_MAX_LINES: usize = 400;

static LAST_WEBVIEW_OK_MS: AtomicI64 = AtomicI64::new(0);
static LAST_WEBVIEW_RECOVERY_MS: AtomicI64 = AtomicI64::new(0);
static LAST_HEARTBEAT_NUDGE_MS: AtomicI64 = AtomicI64::new(0);

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn note_webview_navigation_ok(url: &str) {
    if uce_url_looks_like_chrome_interstitial_error(url) {
        return;
    }
    LAST_WEBVIEW_OK_MS.store(now_unix_ms(), Ordering::Relaxed);
}

fn append_health_log(line: &str) {
    if let Some(parent) = std::path::Path::new(LOG_PATH).parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = match OpenOptions::new().create(true).append(true).open(LOG_PATH) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("UCE_HEALTH_LOG_OPEN_ERR {e}");
            return;
        }
    };
    let ts = now_unix_ms();
    let _ = writeln!(file, "[{ts}] {line}");
    trim_log_if_huge();
}

fn trim_log_if_huge() {
    let Ok(raw) = fs::read_to_string(LOG_PATH) else {
        return;
    };
    let line_count = raw.lines().count();
    if line_count <= LOG_MAX_LINES {
        return;
    }
    let keep: Vec<&str> = raw.lines().skip(line_count - LOG_MAX_LINES).collect();
    let _ = fs::write(LOG_PATH, keep.join("\n"));
    let _ = fs::OpenOptions::new().append(true).open(LOG_PATH);
}

fn tenant_configured(app: &AppHandle) -> bool {
    let cfg = tenant_config::load_tenant_config(app).unwrap_or_default();
    !cfg.business_id.trim().is_empty()
        && !cfg.backend_url.trim().is_empty()
        && !cfg.anon_key.trim().is_empty()
}

pub fn install_panic_reporter() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("panic: {}", info);
        device_health::set_last_error(&msg);
        append_health_log(&msg);
        eprintln!("UCE_PANIC {}", msg);
        default(info);
    }));
}

pub fn spawn_health_watchdog(app: AppHandle) {
    install_panic_reporter();
    std::thread::spawn(move || {
        append_health_log("watchdog started");
        loop {
            std::thread::sleep(Duration::from_secs(CHECK_INTERVAL_SECS));
            run_watchdog_tick(&app);
        }
    });
}

fn run_watchdog_tick(app: &AppHandle) {
    if !tenant_configured(app) {
        return;
    }
    if ccc_package_sync::is_sync_paused() {
        return;
    }

    let now = now_unix_ms();
    let hb = connection_diagnostics::heartbeat_outcome(app);
    let hb_age = if hb.last_unix_ms > 0 {
        now.saturating_sub(hb.last_unix_ms)
    } else {
        i64::MAX
    };

    if hb.last_unix_ms > 0 && hb_age > HEARTBEAT_NUDGE_MS {
        let last_nudge = LAST_HEARTBEAT_NUDGE_MS.load(Ordering::Relaxed);
        if now.saturating_sub(last_nudge) > HEARTBEAT_NUDGE_MS {
            LAST_HEARTBEAT_NUDGE_MS.store(now, Ordering::Relaxed);
            append_health_log(&format!(
                "heartbeat stale age_ms={hb_age} category={} — nudge JS",
                hb.category
            ));
            eprintln!("UCE_WATCHDOG_HEARTBEAT_NUDGE age_ms={hb_age}");
            device_health::set_last_error(format!(
                "heartbeat stale {}m (category {})",
                hb_age / 60_000,
                hb.category
            ));
            let _ = app.emit("uce:watchdog-heartbeat-nudge", hb_age);
        }
    }

    let ccc_last = device_health::last_ccc_sync_unix_ms();
    if ccc_last > 0 {
        let ccc_age = now.saturating_sub(ccc_last);
        if ccc_age > CCC_ACTIVITY_STALE_MS && !ccc_package_sync::is_ccc_offline() {
            append_health_log(&format!(
                "ccc sync idle long age_ms={ccc_age} (poll continues)"
            ));
        }
    }

    let app_wv = app.clone();
    let _ = app.run_on_main_thread(move || {
        check_webview_on_main(&app_wv);
    });

    device_health::refresh_tray(app);
}

fn check_webview_on_main(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let Ok(current) = w.url() else {
        return;
    };
    let url_str = current.as_str();
    if uce_url_looks_like_chrome_interstitial_error(url_str) {
        let now = now_unix_ms();
        let last = LAST_WEBVIEW_RECOVERY_MS.load(Ordering::Relaxed);
        if now.saturating_sub(last) < WEBVIEW_RECOVERY_COOLDOWN_MS {
            return;
        }
        LAST_WEBVIEW_RECOVERY_MS.store(now, Ordering::Relaxed);
        append_health_log(&format!("webview chrome-error — recovery url={url_str}"));
        eprintln!("UCE_WATCHDOG_WEBVIEW_RECOVERY {}", url_str);
        uce_run_chrome_error_recovery_or_alert(app.clone());
        return;
    }
    note_webview_navigation_ok(url_str);
}
