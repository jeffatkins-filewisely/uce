//! JS-reported runtime state for Connection Doctor (listener registration, incoming events, skips, popups).

use serde::Serialize;
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[derive(Clone, Serialize, Default)]
pub struct JsRuntimeDiagSnapshot {
    pub suppress_all_popups: bool,
    pub js_upload_listener_registered_unix_ms: Option<i64>,
    pub last_js_incoming_pdf_at_unix_ms: Option<i64>,
    pub last_js_incoming_pdf_path: Option<String>,
    pub last_upload_skip_reason: Option<String>,
    pub last_popup_kind: Option<String>,
    pub last_popup_source: Option<String>,
    pub last_popup_message_preview: Option<String>,
    pub last_popup_at_unix_ms: Option<i64>,
    pub last_popup_suppressed_kind: Option<String>,
    pub last_popup_suppressed_source: Option<String>,
    pub last_popup_suppressed_message_preview: Option<String>,
    pub last_popup_suppressed_at_unix_ms: Option<i64>,
    /// Every native dialog attempt hits this **before** suppression / `MessageBoxW` (`native_message_box`).
    pub last_native_popup_attempt_kind: Option<String>,
    pub last_native_popup_attempt_source: Option<String>,
    pub last_native_popup_attempt_message_preview: Option<String>,
    pub last_native_popup_attempt_at_unix_ms: Option<i64>,
    /// Rust `MessageBoxW` only (`native_message_box` module).
    pub last_native_popup_kind: Option<String>,
    pub last_native_popup_source: Option<String>,
    pub last_native_popup_message_preview: Option<String>,
    pub last_native_popup_at_unix_ms: Option<i64>,
    pub last_native_popup_suppressed_kind: Option<String>,
    pub last_native_popup_suppressed_source: Option<String>,
    pub last_native_popup_suppressed_message_preview: Option<String>,
    pub last_native_popup_suppressed_at_unix_ms: Option<i64>,
}

static SNAP: LazyLock<Mutex<JsRuntimeDiagSnapshot>> =
    LazyLock::new(|| Mutex::new(JsRuntimeDiagSnapshot::default()));

pub fn snapshot() -> JsRuntimeDiagSnapshot {
    let mut g = SNAP.lock().map(|x| x.clone()).unwrap_or_default();
    g.suppress_all_popups = super::popup_suppression::suppress_all_effective();
    g
}

pub fn set_upload_listener_ready() {
    if let Ok(mut g) = SNAP.lock() {
        g.js_upload_listener_registered_unix_ms = Some(now_ms());
    }
}

pub fn record_incoming_pdf_event(path: String) {
    if let Ok(mut g) = SNAP.lock() {
        g.last_js_incoming_pdf_at_unix_ms = Some(now_ms());
        g.last_js_incoming_pdf_path = Some(path);
    }
}

pub fn record_upload_skip(reason: String) {
    if let Ok(mut g) = SNAP.lock() {
        g.last_upload_skip_reason = Some(reason);
    }
}

pub fn record_popup(kind: String, source: String, message_preview: Option<String>) {
    if let Ok(mut g) = SNAP.lock() {
        g.last_popup_kind = Some(kind);
        g.last_popup_source = Some(source);
        g.last_popup_message_preview = message_preview;
        g.last_popup_at_unix_ms = Some(now_ms());
    }
}

pub fn record_popup_suppressed(kind: String, source: String, message_preview: Option<String>) {
    if let Ok(mut g) = SNAP.lock() {
        g.last_popup_suppressed_kind = Some(kind);
        g.last_popup_suppressed_source = Some(source);
        g.last_popup_suppressed_message_preview = message_preview;
        g.last_popup_suppressed_at_unix_ms = Some(now_ms());
    }
}

pub fn record_native_popup_attempt(kind: String, source: String, message_preview: String) {
    if let Ok(mut g) = SNAP.lock() {
        g.last_native_popup_attempt_kind = Some(kind);
        g.last_native_popup_attempt_source = Some(source);
        g.last_native_popup_attempt_message_preview = Some(message_preview);
        g.last_native_popup_attempt_at_unix_ms = Some(now_ms());
    }
}

pub fn record_native_popup_shown(kind: String, source: String, message_preview: String) {
    if let Ok(mut g) = SNAP.lock() {
        g.last_native_popup_kind = Some(kind);
        g.last_native_popup_source = Some(source);
        g.last_native_popup_message_preview = Some(message_preview);
        g.last_native_popup_at_unix_ms = Some(now_ms());
    }
}

pub fn record_native_popup_suppressed(kind: String, source: String, message_preview: String) {
    if let Ok(mut g) = SNAP.lock() {
        g.last_native_popup_suppressed_kind = Some(kind);
        g.last_native_popup_suppressed_source = Some(source);
        g.last_native_popup_suppressed_message_preview = Some(message_preview);
        g.last_native_popup_suppressed_at_unix_ms = Some(now_ms());
    }
}
