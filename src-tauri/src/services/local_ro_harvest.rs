//! One-time / on-demand pull of local RO documents **up** to FileWisely.
//!
//! Walks shop-local folders (CCC workfiles, FileWisely Scans, extra watch dirs,
//! Desktop/Documents folders that look like ROs). Skips `C:\FileWisely\CCC Import`
//! — that tree is the **download** mirror, not a source of new uploads.

use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

use crate::config::print_config;
use crate::pdf_watch_config;
use crate::services::converter;
use crate::services::print_watcher;

const MAX_FILES: usize = 150;
const MAX_DEPTH: usize = 4;
const MAX_VISITS: usize = 4000;

#[derive(Debug, Clone, Serialize)]
pub struct LocalRoHarvestSummary {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub queued: Vec<String>,
    pub skipped_seen: usize,
    pub skipped_mirror: usize,
    pub visits: usize,
    pub roots: Vec<String>,
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn looks_like_ro_folder_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let t = lower.trim();
    t.starts_with("ro")
        || t.contains("ro-")
        || t.contains("ro_")
        || t.contains("ro ")
        || regex_ro_digits(t)
}

fn regex_ro_digits(name: &str) -> bool {
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i - start >= 4 && i - start <= 7 {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

fn is_harvest_ext(path: &Path) -> Option<&'static str> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "pdf" => Some("pdf"),
        "doc" | "docx" | "rtf" => Some("office"),
        "jpg" | "jpeg" | "png" | "tif" | "tiff" | "bmp" => Some("image"),
        _ => None,
    }
}

fn is_ccc_import_mirror(path: &Path) -> bool {
    let s = path.to_string_lossy().to_lowercase();
    s.contains("\\filewisely\\ccc import") || s.contains("/filewisely/ccc import")
}

fn seen_path(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    let _ = fs::create_dir_all(&dir);
    Some(dir.join("uce-local-ro-harvest-seen.json"))
}

fn load_seen(app: &AppHandle) -> HashSet<String> {
    let Some(p) = seen_path(app) else {
        return HashSet::new();
    };
    let Ok(raw) = fs::read_to_string(p) else {
        return HashSet::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_seen(app: &AppHandle, seen: &HashSet<String>) {
    let Some(p) = seen_path(app) else {
        return;
    };
    if let Ok(raw) = serde_json::to_string_pretty(seen) {
        let _ = fs::write(p, raw);
    }
}

fn file_key(path: &Path) -> String {
    let len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mt = fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{}:{}:{}", path.to_string_lossy().to_lowercase(), len, mt)
}

fn collect_roots(app: &AppHandle) -> Vec<PathBuf> {
    let mut out = Vec::new();
    out.push(PathBuf::from(print_config::FW_SCANS_DIR));
    let incoming = PathBuf::from(print_config::FW_OUTPUT_DIR);
    out.push(incoming);
    out.push(PathBuf::from(r"C:\CCC\WORKFILES"));
    out.push(PathBuf::from(r"C:\CCC"));
    if let Ok(pd) = std::env::var("PROGRAMDATA") {
        out.push(PathBuf::from(pd).join("CCCInformation Services"));
    }
    let cfg = pdf_watch_config::load_pdf_watch_config(app);
    for s in cfg
        .extra_dirs
        .iter()
        .chain(cfg.auto_discovered_ccc_dirs.iter())
        .chain(cfg.auto_discovered_source_dirs.iter())
    {
        let p = PathBuf::from(s.trim());
        if !p.as_os_str().is_empty() {
            out.push(p);
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let base = PathBuf::from(home);
        for parent in [base.join("Desktop"), base.join("Documents")] {
            if let Ok(rd) = fs::read_dir(&parent) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        if looks_like_ro_folder_name(
                            &e.file_name().to_string_lossy(),
                        ) {
                            out.push(p);
                        }
                    }
                }
            }
        }
    }
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for p in out {
        if is_ccc_import_mirror(&p) {
            continue;
        }
        let k = p.to_string_lossy().to_lowercase();
        if seen.insert(k) {
            deduped.push(p);
        }
    }
    deduped
}

struct WalkState {
    files: Vec<(PathBuf, SystemTime)>,
    visits: usize,
}

fn walk(root: &Path, depth: usize, st: &mut WalkState) {
    if st.visits >= MAX_VISITS || depth > MAX_DEPTH || st.files.len() >= MAX_FILES * 3 {
        return;
    }
    if !root.exists() || !root.is_dir() || is_ccc_import_mirror(root) {
        return;
    }
    st.visits += 1;
    let Ok(rd) = fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        if st.visits >= MAX_VISITS {
            return;
        }
        let path = e.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if name.eq_ignore_ascii_case(".uce_staging")
                || name.eq_ignore_ascii_case("processed")
                || name.eq_ignore_ascii_case("failed")
            {
                continue;
            }
            walk(&path, depth + 1, st);
        } else if path.is_file() {
            st.visits += 1;
            if is_harvest_ext(&path).is_none() {
                continue;
            }
            if converter::path_is_under_uce_staging(&path) {
                continue;
            }
            if is_ccc_import_mirror(&path) {
                continue;
            }
            let mtime = fs::metadata(&path)
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            st.files.push((path, mtime));
        }
    }
}

pub fn run_for_app(app: &AppHandle) -> LocalRoHarvestSummary {
    #[cfg(not(windows))]
    {
        let _ = app;
        return LocalRoHarvestSummary {
            ok: false,
            error: Some("Local RO harvest is Windows-only".into()),
            queued: vec![],
            skipped_seen: 0,
            skipped_mirror: 0,
            visits: 0,
            roots: vec![],
        };
    }
    #[cfg(windows)]
    {
        run_inner(app)
    }
}

#[cfg(windows)]
fn run_inner(app: &AppHandle) -> LocalRoHarvestSummary {
    eprintln!("UCE_LOCAL_RO_HARVEST_STARTED");
    let roots = collect_roots(app);
    let root_labels: Vec<String> = roots
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let mut st = WalkState {
        files: Vec::new(),
        visits: 0,
    };
    for r in &roots {
        if r.exists() {
            walk(r, 0, &mut st);
        }
    }
    st.files.sort_by(|a, b| b.1.cmp(&a.1));

    let mut seen = load_seen(app);
    let mut queued = Vec::new();
    let mut skipped_seen = 0usize;
    let mut skipped_mirror = 0usize;

    for (path, _) in st.files {
        if queued.len() >= MAX_FILES {
            break;
        }
        if is_ccc_import_mirror(&path) {
            skipped_mirror += 1;
            continue;
        }
        let key = file_key(&path);
        if seen.contains(&key) {
            skipped_seen += 1;
            continue;
        }
        let kind = is_harvest_ext(&path);
        let to_ingest = match kind {
            Some("image") | Some("office") => {
                let dest_dir = if kind == Some("image") {
                    PathBuf::from(print_config::FW_SCANS_DIR)
                } else {
                    PathBuf::from(print_config::FW_OUTPUT_DIR)
                };
                let _ = fs::create_dir_all(&dest_dir);
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("bin");
                let dest = dest_dir.join(format!("harvest_{}.{}", now_unix_ms(), ext));
                match fs::copy(&path, &dest) {
                    Ok(_) => dest,
                    Err(e) => {
                        eprintln!(
                            "UCE_LOCAL_RO_HARVEST_COPY_FAIL src={} err={e}",
                            path.display()
                        );
                        continue;
                    }
                }
            }
            Some(_) => path.clone(),
            None => continue,
        };
        print_watcher::ingest_path_now(app, to_ingest);
        seen.insert(key);
        queued.push(path.to_string_lossy().to_string());
        eprintln!("UCE_LOCAL_RO_HARVEST_QUEUED path={}", path.display());
    }

    save_seen(app, &seen);
    eprintln!(
        "UCE_LOCAL_RO_HARVEST_COMPLETE queued={} skipped_seen={} visits={}",
        queued.len(),
        skipped_seen,
        st.visits
    );
    LocalRoHarvestSummary {
        ok: true,
        error: None,
        queued,
        skipped_seen,
        skipped_mirror,
        visits: st.visits,
        roots: root_labels,
    }
}
