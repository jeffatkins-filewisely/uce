//! Foreground WINWORD telemetry and optional “send to FileWisely” prompt when the title contains a path.

#[cfg(windows)]
use active_win_pos_rs::get_active_window;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::sync::{Mutex, OnceLock};
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
use serde_json::json;
#[cfg(windows)]
use tauri::{AppHandle, Emitter};

#[cfg(windows)]
use crate::pdf_watch_config;

#[cfg(windows)]
static LAST_WINWORD_PROMPT: OnceLock<Mutex<Option<(String, Instant)>>> = OnceLock::new();

#[cfg(windows)]
fn last_winword_prompt_slot() -> &'static Mutex<Option<(String, Instant)>> {
    LAST_WINWORD_PROMPT.get_or_init(|| Mutex::new(None))
}

#[cfg(windows)]
fn word_title_document_core(title: &str) -> &str {
    let t = title.trim();
    for suf in [
        " - Word",
        " – Word",
        " - Microsoft Word",
        " – Microsoft Word",
    ] {
        if let Some(p) = t.strip_suffix(suf) {
            return p.trim();
        }
    }
    t
}

/// If the Word window title is a full path to an Office file, return it when the file exists.
#[cfg(windows)]
pub fn extract_path_from_word_title(title: &str) -> Option<PathBuf> {
    let core = word_title_document_core(title);
    if core.len() < 5 {
        return None;
    }
    let b = core.as_bytes();
    let looks_abs = b.len() >= 3 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
        || core.starts_with("\\\\");
    if !looks_abs {
        return None;
    }
    let p = PathBuf::from(core);
    if !p.is_file() {
        return None;
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())?;
    if !matches!(ext.as_str(), "doc" | "docx" | "rtf") {
        return None;
    }
    Some(p)
}

#[cfg(windows)]
fn should_emit_winword_prompt(path_key: &str) -> bool {
    const COOLDOWN: Duration = Duration::from_secs(90);
    let now = Instant::now();
    let Ok(mut g) = last_winword_prompt_slot().lock() else {
        return true;
    };
    if let Some((k, t)) = g.as_ref() {
        if k == path_key && now.duration_since(*t) < COOLDOWN {
            return false;
        }
    }
    *g = Some((path_key.to_string(), now));
    true
}

#[cfg(windows)]
pub fn spawn_office_winword_telemetry(app: AppHandle) {
    thread::spawn(move || {
        let mut last_log_key: Option<String> = None;
        loop {
            thread::sleep(Duration::from_secs(2));
            let w = match get_active_window() {
                Ok(w) => w,
                Err(_) => continue,
            };
            let exe = w
                .process_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !exe.contains("winword") {
                continue;
            }

            let title_full = w.title.trim().to_string();
            let title_short: String = title_full.chars().take(120).collect();
            let pid = w.process_id;
            let proc_path = w.process_path.display().to_string();
            let resolved = extract_path_from_word_title(&title_full);
            let path_str = resolved
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            let log_key = format!("{}|{}", pid, title_short);
            if last_log_key.as_ref() != Some(&log_key) {
                last_log_key = Some(log_key.clone());
                eprintln!(
                    "[UCE] OFFICE_WINWORD_DETECTED OFFICE_WINWORD_TITLE={} OFFICE_WINWORD_PID={} OFFICE_WINWORD_PATH={} OFFICE_WINWORD_PROCESS_PATH={}",
                    title_short,
                    pid,
                    path_str,
                    proc_path
                );
            }

            let cfg = pdf_watch_config::load_pdf_watch_config(&app);
            if !cfg.office_winword_send_prompt {
                continue;
            }
            let Some(doc_path) = resolved else {
                continue;
            };
            let key = doc_path.to_string_lossy().to_lowercase();
            if !should_emit_winword_prompt(&key) {
                continue;
            }
            let _ = app.emit(
                "uce-filewisely-send-doc-prompt",
                json!({
                    "path": doc_path.to_string_lossy().to_string(),
                    "source": "winword_title",
                }),
            );
        }
    });
}

#[cfg(not(windows))]
pub fn spawn_office_winword_telemetry(_app: tauri::AppHandle) {}
