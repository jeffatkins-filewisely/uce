//! Discover folders where this PC prints and scans, then persist them into
//! `uce-pdf-watch.json` → `auto_discovered_source_dirs`.
//!
//! `\\.\Usbscan0` is a WIA **device port**, not a folder. CCC “Attach from Camera/Scanner”
//! writes a temp/image file somewhere else (often Pictures\Scanned Documents, a vendor
//! folder, or `%TEMP%`). This module:
//! 1. Seeds known Windows / Epson / Brother scan destinations
//! 2. Watches foreground titles for WIA / print-to-PDF dialogs
//! 3. Harvests newly created PDF/image files and learns their parent folders
//! 4. Copies `%TEMP%` harvests into `C:\FileWisely\Scans` so the watcher can ingest them

use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

use crate::config::print_config;
use crate::context_tracker;
use crate::pdf_watch_config;
use crate::services::print_watcher;

const RECENT_SECS: u64 = 2 * 24 * 3600;
const HARVEST_RECENT_SECS: u64 = 120;
const HARVEST_WINDOW_SECS: u64 = 45;
const MAX_DISCOVERED: usize = 40;

#[derive(Debug, Clone, Serialize)]
pub struct SourceAutodiscoverySummary {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub added: Vec<String>,
    pub candidates: Vec<String>,
    pub devices: Vec<String>,
    pub no_new_dirs: bool,
    pub trigger: String,
}

struct LastState {
    last_run_unix_ms: i64,
    candidates: Vec<String>,
    last_added: Vec<String>,
    devices: Vec<String>,
    last_dialog: Option<String>,
}

static LAST: Mutex<Option<LastState>> = Mutex::new(None);

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn is_recent_mtime(t: SystemTime, max_age: Duration) -> bool {
    SystemTime::now()
        .duration_since(t)
        .map(|d| d <= max_age)
        .unwrap_or(false)
}

pub fn is_scan_or_print_dialog(app_name: &str, title: &str) -> bool {
    let hay = format!("{app_name} {title}").to_lowercase();
    const NEEDLES: &[&str] = &[
        "scan using",
        "acquiring data",
        "which device do you want to use",
        "windows image acquisition",
        "wia",
        "twain",
        "epson ds",
        "epson scan",
        "brother",
        "hp scan",
        "scansnap",
        "canon",
        "attach - from camera",
        "attach from camera",
        "camera/scanner",
        "windows fax and scan",
        "save print output as",
        "print to pdf",
        "filewisely printer",
        "bullzip",
    ];
    NEEDLES.iter().any(|n| hay.contains(n))
}

fn is_capture_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_lowercase().as_str(),
                "pdf" | "jpg" | "jpeg" | "png" | "tif" | "tiff" | "bmp"
            )
        })
        .unwrap_or(false)
}

fn looks_like_scan_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    name.contains("scan")
        || name.contains("wia")
        || name.contains("epson")
        || name.contains("brother")
        || name.contains("scansnap")
        || name.contains("canon")
        || name.contains("hp scan")
        || name.contains("twain")
        || name.contains("img")
        || name.starts_with("image")
        || name.contains("document")
}

fn safe_persistable_folder(p: &Path) -> bool {
    let s = p.to_string_lossy().to_lowercase();
    if s.contains("\\windows\\") || s.starts_with("c:\\windows") {
        return false;
    }
    if s.contains("\\program files\\") || s.contains("\\program files (x86)\\") {
        return false;
    }
    if s.ends_with("\\temp") || s.ends_with("\\tmp") || s.ends_with("\\appdata\\local\\temp") {
        return false;
    }
    if s.contains("\\appdata\\local\\temp\\") && !s.contains("\\temp\\ccc") {
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        return name.contains("wia")
            || name.contains("epson")
            || name.contains("scan")
            || name.contains("twain")
            || name.contains("brother");
    }
    true
}

fn collect_known_scan_roots() -> Vec<PathBuf> {
    pdf_watch_config::scan_destination_roots_with_rules()
        .into_iter()
        .map(|(p, _)| p)
        .collect()
}

fn collect_harvest_roots(app: &AppHandle) -> Vec<PathBuf> {
    let mut v = collect_known_scan_roots();
    if let Ok(home) = std::env::var("USERPROFILE") {
        let base = PathBuf::from(&home);
        v.push(base.join("Pictures"));
        v.push(base.join("Documents"));
        v.push(base.join("Downloads"));
        v.push(base.join("Desktop"));
    }
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        v.push(PathBuf::from(la).join("Temp"));
    }
    if let Ok(tmp) = std::env::var("TEMP") {
        v.push(PathBuf::from(tmp));
    }
    v.push(print_config::ccc_temp_watch_path());
    v.push(PathBuf::from(print_config::FW_OUTPUT_DIR));
    v.push(PathBuf::from(print_config::FW_SCANS_DIR));

    let cfg = pdf_watch_config::load_pdf_watch_config(app);
    for s in cfg
        .extra_dirs
        .iter()
        .chain(cfg.auto_discovered_source_dirs.iter())
        .chain(cfg.auto_discovered_ccc_dirs.iter())
    {
        let p = PathBuf::from(s.trim());
        if !p.as_os_str().is_empty() {
            v.push(p);
        }
    }
    dedupe_paths(v)
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in paths {
        let k = p.to_string_lossy().to_lowercase();
        if seen.insert(k) {
            out.push(p);
        }
    }
    out
}

fn list_recent_capture_files(root: &Path, max_age: Duration, out: &mut Vec<PathBuf>) {
    if !root.exists() || !root.is_dir() {
        return;
    }
    let Ok(rd) = fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        if !path.is_file() || !is_capture_ext(&path) {
            continue;
        }
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        if !is_recent_mtime(mtime, max_age) {
            continue;
        }
        if meta.len() < 64 {
            continue;
        }
        out.push(path);
    }
}

fn persist_folders(app: &AppHandle, folders: &[PathBuf]) -> Vec<String> {
    let mut cfg = pdf_watch_config::load_pdf_watch_config(app);
    let before = cfg.auto_discovered_source_dirs.clone();
    let mut merged = before.clone();
    let mut added = Vec::new();
    for p in folders {
        if !safe_persistable_folder(p) {
            continue;
        }
        let s = p.to_string_lossy().to_string();
        if merged.iter().any(|x| x.eq_ignore_ascii_case(s.trim())) {
            continue;
        }
        if merged.len() >= MAX_DISCOVERED {
            break;
        }
        merged.push(s.clone());
        added.push(s);
    }
    if merged != before {
        cfg.auto_discovered_source_dirs = merged;
        if let Err(e) = pdf_watch_config::save_pdf_watch_config(app, &cfg) {
            eprintln!("UCE_SOURCE_AUTODISCOVERY_SAVE_FAILED err={e}");
        }
    }
    added
}

fn copy_temp_harvest_to_scans(src: &Path) -> Option<PathBuf> {
    let lower = src.to_string_lossy().to_lowercase();
    let is_temp = lower.contains("\\appdata\\local\\temp\\") || lower.contains("\\temp\\");
    if !is_temp {
        return None;
    }
    if lower.contains("\\temp\\ccc") {
        return None;
    }
    let dest_dir = PathBuf::from(print_config::FW_SCANS_DIR);
    if fs::create_dir_all(&dest_dir).is_err() {
        return None;
    }
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");
    let ts = now_unix_ms();
    let dest = dest_dir.join(format!("wia_{ts}.{ext}"));
    match fs::copy(src, &dest) {
        Ok(_) => {
            eprintln!(
                "UCE_SOURCE_HARVEST_COPIED src={} dest={}",
                src.display(),
                dest.display()
            );
            Some(dest)
        }
        Err(e) => {
            eprintln!(
                "UCE_SOURCE_HARVEST_COPY_FAILED src={} err={e}",
                src.display()
            );
            None
        }
    }
}

fn should_ingest_harvest(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_lowercase();
    if lower.contains("\\scanned documents")
        || lower.contains("\\epson")
        || lower.contains("\\brother")
        || lower.contains("\\fax")
        || lower.contains("filewisely\\scans")
        || lower.contains("filewisely\\incoming")
        || lower.contains("\\temp\\ccc")
    {
        return true;
    }
    if lower.contains("\\appdata\\local\\temp\\") || lower.contains("\\temp\\") {
        return true;
    }
    looks_like_scan_name(path)
}

fn offer_discovered_file(app: &AppHandle, path: &Path) {
    if !should_ingest_harvest(path) {
        return;
    }
    if let Some(copied) = copy_temp_harvest_to_scans(path) {
        print_watcher::ingest_path_now(app, copied);
        return;
    }
    print_watcher::ingest_path_now(app, path.to_path_buf());
}

fn enumerate_scan_devices() -> Vec<String> {
    #[cfg(not(windows))]
    {
        return vec![];
    }
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            r#"Get-PnpDevice -Class Image,Printer -Status OK -ErrorAction SilentlyContinue | Select-Object -ExpandProperty FriendlyName"#,
        ])
        .stdin(std::process::Stdio::null());
        crate::services::process_launch::apply_hidden(&mut cmd);
        match crate::services::process_launch::run_output(
            "source_autodiscovery",
            "list_image_printer_devices",
            cmd,
            Duration::from_secs(12),
        ) {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .take(24)
                .collect(),
            _ => vec![],
        }
    }
}

pub fn diagnostics_snapshot() -> serde_json::Value {
    let g = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let Some(st) = g.as_ref() else {
        return serde_json::json!({
            "source_autodiscovery_last_run": serde_json::Value::Null,
            "source_autodiscovery_candidates": [],
            "source_autodiscovery_last_added": [],
            "source_autodiscovery_devices": [],
        });
    };
    serde_json::json!({
        "source_autodiscovery_last_run_unix_ms": st.last_run_unix_ms,
        "source_autodiscovery_candidates": st.candidates,
        "source_autodiscovery_last_added": st.last_added,
        "source_autodiscovery_devices": st.devices,
        "source_autodiscovery_last_dialog": st.last_dialog,
    })
}

pub fn run_for_app(app: &AppHandle, trigger: &str) -> SourceAutodiscoverySummary {
    #[cfg(not(windows))]
    {
        let _ = app;
        return SourceAutodiscoverySummary {
            ok: false,
            error: Some("Source autodiscovery is Windows-only".into()),
            added: vec![],
            candidates: vec![],
            devices: vec![],
            no_new_dirs: true,
            trigger: trigger.to_string(),
        };
    }

    #[cfg(windows)]
    {
        run_for_app_inner(app, trigger)
    }
}

#[cfg(windows)]
fn run_for_app_inner(app: &AppHandle, trigger: &str) -> SourceAutodiscoverySummary {
    eprintln!("UCE_SOURCE_AUTODISCOVERY_STARTED trigger={trigger}");
    let _ = fs::create_dir_all(print_config::FW_SCANS_DIR);

    let mut folders: HashSet<PathBuf> = HashSet::new();
    let mut candidates = Vec::new();

    for p in collect_known_scan_roots() {
        if p.exists() {
            folders.insert(p.clone());
            candidates.push(p.to_string_lossy().to_string());
        }
    }

    let harvest_roots = collect_harvest_roots(app);
    let max_age = Duration::from_secs(RECENT_SECS);
    let mut recent_files = Vec::new();
    for root in &harvest_roots {
        list_recent_capture_files(root, max_age, &mut recent_files);
    }
    for path in recent_files {
        if looks_like_scan_name(&path) || is_scan_or_print_dialog("", &path.to_string_lossy()) {
            if let Some(parent) = path.parent() {
                if safe_persistable_folder(parent) {
                    folders.insert(parent.to_path_buf());
                    candidates.push(parent.to_string_lossy().to_string());
                }
            }
        }
    }

    let devices = enumerate_scan_devices();
    let added = persist_folders(app, &folders.into_iter().collect::<Vec<_>>());
    let candidates_trim: Vec<String> = {
        let mut v = candidates;
        v.sort();
        v.dedup();
        v.into_iter().take(120).collect()
    };

    {
        let mut g = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let prev_dialog = g.as_ref().and_then(|s| s.last_dialog.clone());
        *g = Some(LastState {
            last_run_unix_ms: now_unix_ms(),
            candidates: candidates_trim.clone(),
            last_added: added.clone(),
            devices: devices.clone(),
            last_dialog: prev_dialog,
        });
    }

    eprintln!(
        "UCE_SOURCE_AUTODISCOVERY_COMPLETE added_count={} devices={}",
        added.len(),
        devices.len()
    );

    SourceAutodiscoverySummary {
        ok: true,
        error: None,
        no_new_dirs: added.is_empty(),
        added,
        candidates: candidates_trim,
        devices,
        trigger: trigger.to_string(),
    }
}

/// Background loop: when a WIA / print dialog is in the foreground, harvest new files
/// for ~45s and learn their parent folders.
pub fn spawn_source_harvest_loop(app: AppHandle) {
    #[cfg(not(windows))]
    {
        let _ = app;
    }
    #[cfg(windows)]
    {
        let res = thread::Builder::new()
            .name("uce-source-harvest".into())
            .spawn(move || harvest_loop(app));
        if let Err(e) = res {
            eprintln!("UCE_SOURCE_HARVEST_THREAD_FAILED err={e}");
        }
    }
}

#[cfg(windows)]
fn harvest_loop(app: AppHandle) {
    eprintln!("UCE_SOURCE_HARVEST_STARTED interval_secs=2 window_secs={HARVEST_WINDOW_SECS}");
    let mut harvest_until: Option<Instant> = None;
    let mut seen: HashSet<String> = HashSet::new();
    loop {
        thread::sleep(Duration::from_secs(2));
        let (app_name, title) = context_tracker::current_window_info();
        if is_scan_or_print_dialog(&app_name, &title) {
            harvest_until = Some(Instant::now() + Duration::from_secs(HARVEST_WINDOW_SECS));
            {
                let mut g = LAST.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(st) = g.as_mut() {
                    st.last_dialog = Some(format!("{app_name} | {title}"));
                } else {
                    *g = Some(LastState {
                        last_run_unix_ms: now_unix_ms(),
                        candidates: vec![],
                        last_added: vec![],
                        devices: vec![],
                        last_dialog: Some(format!("{app_name} | {title}")),
                    });
                }
            }
            eprintln!("UCE_SOURCE_DIALOG_SEEN app={app_name} title={title}");
        }
        let Some(until) = harvest_until else {
            continue;
        };
        if Instant::now() > until {
            harvest_until = None;
            continue;
        }

        let mut files = Vec::new();
        for root in collect_harvest_roots(&app) {
            list_recent_capture_files(&root, Duration::from_secs(HARVEST_RECENT_SECS), &mut files);
        }
        let mut learned = Vec::new();
        for path in files {
            let key = format!(
                "{}:{}",
                path.to_string_lossy().to_lowercase(),
                fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
            );
            if !seen.insert(key) {
                continue;
            }
            if let Some(parent) = path.parent() {
                if safe_persistable_folder(parent) {
                    learned.push(parent.to_path_buf());
                }
            }
            offer_discovered_file(&app, &path);
        }
        if !learned.is_empty() {
            let added = persist_folders(&app, &learned);
            if !added.is_empty() {
                eprintln!(
                    "UCE_SOURCE_HARVEST_LEARNED count={} first={}",
                    added.len(),
                    added.first().unwrap_or(&String::new())
                );
            }
        }
        if seen.len() > 400 {
            seen.clear();
        }
    }
}
