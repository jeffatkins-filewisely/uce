//! Main-thread emit of `uce-incoming-file` for the JS upload pipeline (reliable from worker threads).

use serde::Serialize;
use std::path::Path;
use tauri::Emitter;

#[derive(Clone, Serialize)]
pub struct IncomingFileEvent {
    pub path: String,
    pub kind: &'static str,
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
    };

    if let Err(e) = main_thread.run_on_main_thread(move || {
        if trace {
            eprintln!(
                "[UCE] trace uce_incoming_emit_dispatched_main_thread path={}",
                path_log
            );
        }
        if let Err(e2) = emit_handle.emit("uce-incoming-file", payload) {
            eprintln!("[UCE] emit uce-incoming-file failed: {e2}");
        }
    }) {
        eprintln!("[UCE] run_on_main_thread (uce-incoming-file) failed: {e}");
    }
}
