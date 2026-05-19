//! Foreground + folder provenance for upload payloads (emit-time and read-time).

use crate::pdf_watch_config;
use crate::services::foreground_telemetry;
use serde::Serialize;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

#[derive(Clone, Serialize, Default, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct CaptureContext {
    pub source_app: String,
    pub window_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreground_sampled_at_unix_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watch_folder_rule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_kind: Option<String>,
}

pub fn is_unknown_field(s: &str) -> bool {
    let t = s.trim();
    t.is_empty() || t.eq_ignore_ascii_case("unknown")
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Sample the active foreground window (best effort).
pub fn snapshot_foreground() -> CaptureContext {
    let sampled_at = now_unix_ms();
    if let Some(snap) = foreground_telemetry::foreground_snapshot() {
        let source_app = if snap.app_name.trim().is_empty() {
            if snap.exe_token.trim().is_empty() {
                "unknown".to_string()
            } else {
                snap.exe_token.clone()
            }
        } else {
            snap.app_name.clone()
        };
        let window_title = if snap.title_short.trim().is_empty() {
            "unknown".to_string()
        } else {
            snap.title_short.clone()
        };
        return CaptureContext {
            source_app,
            window_title,
            foreground_sampled_at_unix_ms: Some(sampled_at),
            watch_folder_rule: None,
            trigger_kind: None,
        };
    }
    CaptureContext {
        source_app: "unknown".to_string(),
        window_title: "unknown".to_string(),
        foreground_sampled_at_unix_ms: Some(sampled_at),
        watch_folder_rule: None,
        trigger_kind: None,
    }
}

fn folder_fallback_for_path(path: &Path, watch_folder_rule: &str) -> CaptureContext {
    let basename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("document.pdf")
        .to_string();
    let rule = if watch_folder_rule.is_empty() || watch_folder_rule == "unknown" {
        "watched_folder".to_string()
    } else {
        watch_folder_rule.to_string()
    };
    CaptureContext {
        source_app: format!("folder:{rule}"),
        window_title: basename,
        foreground_sampled_at_unix_ms: None,
        watch_folder_rule: Some(rule),
        trigger_kind: None,
    }
}

fn resolve_watch_folder_rule(
    app: Option<&AppHandle>,
    path: Option<&Path>,
    explicit_rule: Option<&str>,
) -> Option<String> {
    if let Some(r) = explicit_rule {
        if !r.is_empty() && r != "unknown" {
            return Some(r.to_string());
        }
    }
    let (app, path) = (app?, path?);
    let roots = pdf_watch_config::office_intercept_watch_roots(app);
    let rule = pdf_watch_config::resolve_office_source_rule(path, roots.as_slice());
    if rule != "unknown" {
        Some(rule.to_string())
    } else {
        None
    }
}

/// Build context for an incoming/read/upload path: foreground first, folder basename fallback.
pub fn build_capture_context(
    app: Option<&AppHandle>,
    path: Option<&Path>,
    trigger_kind: &str,
    watch_folder_rule: Option<&str>,
) -> CaptureContext {
    let mut ctx = snapshot_foreground();
    ctx.trigger_kind = Some(trigger_kind.to_string());

    if let Some(rule) = resolve_watch_folder_rule(app, path, watch_folder_rule) {
        ctx.watch_folder_rule = Some(rule.clone());
        if is_unknown_field(&ctx.source_app) && is_unknown_field(&ctx.window_title) {
            if let Some(p) = path {
                let fb = folder_fallback_for_path(p, &rule);
                ctx.source_app = fb.source_app;
                ctx.window_title = fb.window_title;
            }
        } else if is_unknown_field(&ctx.window_title) {
            if let Some(p) = path {
                ctx.window_title = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("document.pdf")
                    .to_string();
            }
        }
    } else if is_unknown_field(&ctx.source_app) && is_unknown_field(&ctx.window_title) {
        if let Some(p) = path {
            let fb = folder_fallback_for_path(p, "watched_folder");
            ctx.source_app = fb.source_app;
            ctx.window_title = fb.window_title;
            ctx.watch_folder_rule = fb.watch_folder_rule;
        }
    }

    ctx
}

pub fn log_capture_context(phase: &str, path: &str, ctx: &CaptureContext) {
    eprintln!(
        "[UCE] capture_context_{phase} path={path} source_app={} window_title={} watch_folder_rule={:?} trigger_kind={:?} sampled_at={:?}",
        ctx.source_app,
        ctx.window_title,
        ctx.watch_folder_rule,
        ctx.trigger_kind,
        ctx.foreground_sampled_at_unix_ms
    );
}
