//! Main-thread emit of `uce-incoming-file` for the JS upload pipeline (reliable from worker threads).

use serde::Serialize;
use serde_json::json;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;
use tauri::Emitter;

use crate::services::capture_context::{self, CaptureContext};
use crate::services::pipeline_stage_diag;

#[derive(Clone, Serialize)]
pub struct IncomingFileEvent {
    pub path: String,
    pub kind: &'static str,
    /// Present when file existed at emit time — JS merges into upload batch without relying on `list_pdf_metas_since` (mtime vs `since`) gaps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_unix_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_size: Option<u64>,
    /// Foreground + folder provenance sampled when the file was accepted for upload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_context: Option<CaptureContext>,
}

fn stat_for_emit(path: &Path) -> (Option<i64>, Option<u64>) {
    let Some(meta) = fs::metadata(path).ok() else {
        return (None, None);
    };
    let sz = meta.len();
    let ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64);
    (ms, Some(sz))
}

/// Basename `fw_*.pdf` (case-insensitive) for trace/dedupe/upload bypass.
pub fn is_fw_incoming_pdf_path(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|name| {
            let lower = name.to_lowercase();
            lower.starts_with("fw_") && lower.ends_with(".pdf")
        })
        .unwrap_or(false)
}

pub fn emit_uce_incoming_pdf(app: &tauri::AppHandle, path_str: String) {
    emit_uce_incoming_pdf_detailed(app, path_str, "incoming_pdf", None);
}

pub fn emit_uce_incoming_pdf_detailed(
    app: &tauri::AppHandle,
    path_str: String,
    trigger_kind: &str,
    watch_folder_rule: Option<&str>,
) {
    eprintln!(
        "UCE_RUST_EMIT_INCOMING_BEFORE path={}",
        path_str
    );
    pipeline_stage_diag::record_emit_incoming(&path_str);
    let p = Path::new(&path_str);
    let (modified_unix_ms, file_size) = stat_for_emit(p);
    let capture_context = Some(capture_context::build_capture_context(
        Some(app),
        Some(p),
        trigger_kind,
        watch_folder_rule,
    ));
    if let Some(ref ctx) = capture_context {
        capture_context::log_capture_context("emit", &path_str, ctx);
    }
    eprintln!(
        "UCE_RUST_EMIT_INCOMING path={} event=uce-incoming-file kind=pdf fw_named={} modified_unix_ms={:?} file_size={:?}",
        path_str,
        is_fw_incoming_pdf_path(&path_str),
        modified_unix_ms,
        file_size
    );
    let trace = is_fw_incoming_pdf_path(&path_str);
    if trace {
        let exists = Path::new(&path_str).is_file();
        eprintln!(
            "[UCE] trace local_pdf_created path={} exists={}",
            path_str, exists
        );
        eprintln!(
            "[UCE] trace uce_incoming_emit_requested path={}",
            path_str
        );
    }

    let main_thread = app.clone();
    let emit_handle = app.clone();
    let path_log = path_str.clone();
    let payload = IncomingFileEvent {
        path: path_str,
        kind: "pdf",
        modified_unix_ms,
        file_size,
        capture_context,
    };

    let app_nudge = app.clone();
    let path_nudge = path_log.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = app_nudge.emit("uce-upload-pipeline-nudge", json!({ "path": path_nudge }));
    });

    if let Err(e) = main_thread.run_on_main_thread(move || {
        if trace {
            eprintln!(
                "[UCE] trace uce_incoming_emit_dispatched_main_thread path={}",
                path_log
            );
        }
        if let Err(e2) = emit_handle.emit("uce-incoming-file", payload) {
            eprintln!("[UCE] emit uce-incoming-file failed: {e2}");
        } else {
            eprintln!("UCE_RUST_EMIT_INCOMING_AFTER path={}", path_log);
        }
    }) {
        eprintln!("[UCE] run_on_main_thread (uce-incoming-file) failed: {e}");
    }
}
