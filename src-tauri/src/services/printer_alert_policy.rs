//! Printer MessageBox policy: default **warning-only** (stderr); native dialog only when explicitly required.

use serde::Serialize;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::print_config;
use crate::services::capture_pipeline_status;

static JS_PRINTER_REQUIRED: Mutex<Option<bool>> = Mutex::new(None);

static LAST_NATIVE_PRINTER_ALERT: Mutex<Option<(String, i64)>> = Mutex::new(None);

pub fn set_js_printer_required(v: bool) {
    if let Ok(mut g) = JS_PRINTER_REQUIRED.lock() {
        *g = Some(v);
    }
}

fn js_printer_required() -> Option<bool> {
    JS_PRINTER_REQUIRED.lock().ok().and_then(|g| *g)
}

pub fn env_printer_required() -> bool {
    std::env::var("UCE_PRINTER_REQUIRED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn printer_required_effective() -> bool {
    env_printer_required() || js_printer_required().unwrap_or(false)
}

pub fn ccc_capture_suppresses_native() -> bool {
    print_config::ccc_temp_watch_only() && capture_pipeline_status::is_watcher_running()
}

/// Native MessageBox for missing printer only if user/shop opted in **and** CCC-temp capture is not the active suppressing path.
pub fn native_dialog_allowed_for_missing_printer() -> bool {
    let req = printer_required_effective();
    let ccc_sup = ccc_capture_suppresses_native();
    req && !ccc_sup
}

pub fn alert_policy_label() -> &'static str {
    if native_dialog_allowed_for_missing_printer() {
        "native_when_missing_allowed"
    } else {
        "warning_only"
    }
}

pub fn suppress_printer_severe_native() -> bool {
    !native_dialog_allowed_for_missing_printer()
}

pub fn record_native_printer_alert(kind: &str) {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    if let Ok(mut g) = LAST_NATIVE_PRINTER_ALERT.lock() {
        *g = Some((kind.to_string(), ms));
    }
}

pub fn last_native_alert_kind() -> Option<String> {
    LAST_NATIVE_PRINTER_ALERT
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|(k, _)| k.clone()))
}

pub fn last_native_alert_at_unix_ms() -> Option<i64> {
    LAST_NATIVE_PRINTER_ALERT
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|(_, t)| *t))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterPolicyDiagnostics {
    pub printer_required_mode: bool,
    pub printer_required_env: bool,
    pub printer_required_js_reported: Option<bool>,
    pub printer_alert_policy: &'static str,
    pub ccc_temp_watch_only: bool,
    pub ccc_capture_suppresses_native_printer_alert: bool,
    pub native_dialog_allowed_for_missing_printer: bool,
    pub suppress_printer_severe_native: bool,
    pub last_native_alert_kind: Option<String>,
    pub last_native_alert_at_unix_ms: Option<i64>,
}

pub fn policy_snapshot() -> PrinterPolicyDiagnostics {
    PrinterPolicyDiagnostics {
        printer_required_mode: printer_required_effective(),
        printer_required_env: env_printer_required(),
        printer_required_js_reported: js_printer_required(),
        printer_alert_policy: alert_policy_label(),
        ccc_temp_watch_only: print_config::ccc_temp_watch_only(),
        ccc_capture_suppresses_native_printer_alert: ccc_capture_suppresses_native(),
        native_dialog_allowed_for_missing_printer: native_dialog_allowed_for_missing_printer(),
        suppress_printer_severe_native: suppress_printer_severe_native(),
        last_native_alert_kind: last_native_alert_kind(),
        last_native_alert_at_unix_ms: last_native_alert_at_unix_ms(),
    }
}
