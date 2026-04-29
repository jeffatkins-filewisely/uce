//! Foreground window / process logging for diagnosing shell + Office interference during print capture.

use active_win_pos_rs::get_active_window;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Last time WINWORD was the foreground process (wall clock, unix ms). `0` = never observed.
static LAST_WINWORD_FOREGROUND_UNIX_MS: AtomicU64 = AtomicU64::new(0);

pub fn last_winword_foreground_unix_ms() -> u64 {
    LAST_WINWORD_FOREGROUND_UNIX_MS.load(Ordering::Relaxed)
}

/// True when CCC ONE (or similar) shows a **Printing** dialog — enables burst sweeper fallback.
pub fn ccc_printing_title_active() -> bool {
    match foreground_snapshot() {
        Some(s) => {
            let title = s.title_short.to_lowercase();
            let app = s.app_name.to_lowercase();
            title.contains("printing")
                || (app.contains("ccc") && title.contains("print"))
        }
        None => false,
    }
}

fn record_winword_if_class(snap: &ForegroundSnapshot) {
    if classify_exe(&snap.exe_token.to_lowercase()) == "WINWORD" {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        LAST_WINWORD_FOREGROUND_UNIX_MS.store(ms, Ordering::Relaxed);
    }
}

/// Classify exe name for quick scanning in logs (WINWORD, EXPLORER, SOFFICE, OTHER).
pub fn classify_exe(exe_lower: &str) -> &'static str {
    if exe_lower.contains("winword") {
        "WINWORD"
    } else if exe_lower.contains("explorer") {
        "EXPLORER"
    } else if exe_lower.contains("soffice") || exe_lower.contains("libreoffice") {
        "LIBREOFFICE"
    } else {
        "OTHER"
    }
}

fn exe_token(process_path: &std::path::Path) -> String {
    process_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Snapshot of the current foreground window (active process from OS perspective).
pub struct ForegroundSnapshot {
    pub exe_token: String,
    pub process_id: u64,
    pub app_name: String,
    pub title_short: String,
    pub process_path: std::path::PathBuf,
}

fn truncate_title(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(max).collect::<String>())
    }
}

/// Returns foreground snapshot, or None if unavailable.
pub fn foreground_snapshot() -> Option<ForegroundSnapshot> {
    let w = get_active_window().ok()?;
    let exe_token = exe_token(&w.process_path);
    Some(ForegroundSnapshot {
        exe_token,
        process_id: w.process_id,
        app_name: w.app_name,
        title_short: truncate_title(&w.title, 80),
        process_path: w.process_path,
    })
}

pub fn format_foreground_line(prefix: &str, snap: &ForegroundSnapshot) -> String {
    let path = snap.process_path.display();
    let cls = classify_exe(&snap.exe_token.to_lowercase());
    format!(
        "[UCE] foreground {prefix}: class={cls} exe={} pid={} app_name={} title=\"{}\" path={}",
        snap.exe_token,
        snap.process_id,
        snap.app_name,
        snap.title_short,
        path
    )
}

pub fn log_foreground(prefix: &str) {
    match foreground_snapshot() {
        Some(s) => {
            record_winword_if_class(&s);
            eprintln!("{}", format_foreground_line(prefix, &s));
        }
        None => eprintln!("[UCE] foreground {prefix}: (unavailable)"),
    }
}

/// Short hint when a file claim hits ERROR_SHARING_VIOLATION (likely Word or another app has the file open).
pub fn sharing_violation_process_hint() -> String {
    match foreground_snapshot() {
        Some(s) => {
            record_winword_if_class(&s);
            let cls = classify_exe(&s.exe_token.to_lowercase());
            format!(
                "likely_locker_hint class={cls} exe={} pid={} title=\"{}\"",
                s.exe_token, s.process_id, s.title_short
            )
        }
        None => "likely_locker_hint=(foreground unavailable)".to_string(),
    }
}

const DEBUG_POLL_INTERVAL_MS: u64 = 200;
const DEBUG_POLL_ITERATIONS: u32 = 25;

/// After a watched file is detected, sample the foreground window every 200ms for 5s (25×200ms) on a
/// background thread. Use to see when WINWORD or Explorer becomes active relative to print/save.
pub fn spawn_foreground_debug_poll_after_detection() {
    thread::spawn(|| {
        let mut last_logged: Option<String> = None;
        for _ in 0..DEBUG_POLL_ITERATIONS {
            let line = match foreground_snapshot() {
                Some(s) => {
                    record_winword_if_class(&s);
                    format_foreground_line("debug_poll", &s)
                }
                None => "[UCE] foreground debug_poll: (unavailable)".to_string(),
            };
            if last_logged.as_ref() != Some(&line) {
                eprintln!("{line}");
                last_logged = Some(line);
            }
            thread::sleep(Duration::from_millis(DEBUG_POLL_INTERVAL_MS));
        }
    });
}

#[cfg(windows)]
fn parent_process_line(pid: u32) -> Option<String> {
    let parent_pid: u32 = {
        let mut c = Command::new("powershell.exe");
        c.args([
            "-NoProfile",
            "-Command",
            &format!("(Get-CimInstance Win32_Process -Filter 'ProcessId={pid}').ParentProcessId"),
        ]);
        let out = crate::services::process_launch::run_output(
            "foreground_telemetry",
            "cim_parent_pid",
            c,
            crate::services::process_launch::TIMEOUT_DEFAULT,
        )
        .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()?
    };
    let parent_name = {
        let mut c = Command::new("powershell.exe");
        c.args([
            "-NoProfile",
            "-Command",
            &format!(
                "(Get-CimInstance Win32_Process -Filter 'ProcessId={parent_pid}').Name"
            ),
        ]);
        let out = crate::services::process_launch::run_output(
            "foreground_telemetry",
            "cim_parent_name",
            c,
            crate::services::process_launch::TIMEOUT_DEFAULT,
        )
        .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    Some(format!(
        "[UCE] foreground parent: parent_pid={} parent_exe={}",
        parent_pid, parent_name
    ))
}

#[cfg(not(windows))]
fn parent_process_line(_pid: u32) -> Option<String> {
    None
}

/// After a successful claim: detect→claim ms, claimed paths, foreground + optional parent of foreground PID.
pub fn log_claim_telemetry(
    tag: &str,
    original_incoming: &Path,
    staged_path: &Path,
    detected_at: Option<Instant>,
) {
    let ms = detected_at.map(|t| t.elapsed().as_millis() as u64);
    eprintln!(
        "[UCE] claimed file tag={} original={} staged={} detect_to_claim_ms={}",
        tag,
        original_incoming.display(),
        staged_path.display(),
        ms.map(|m| m.to_string())
            .unwrap_or_else(|| "n/a".to_string())
    );

    if let Some(s) = foreground_snapshot() {
        record_winword_if_class(&s);
        eprintln!("{}", format_foreground_line("at_claim", &s));
        #[cfg(windows)]
        if s.process_id <= u32::MAX as u64 {
            if let Some(line) = parent_process_line(s.process_id as u32) {
                eprintln!("{line}");
            }
        }
    } else {
        eprintln!("[UCE] foreground at_claim: (unavailable)");
    }
}
