//! Extra PDF watch locations (CCC exports, DMS folders) and optional minimum file size.
//!
//! Config file (next to other UCE app data): **`uce-pdf-watch.json`**
//! ```json
//! {
//!   "extra_dirs": ["C:/Shop/CCC PDFs", "C:/FileWisely/Incoming"],
//!   "min_pdf_bytes": 64,
//!   "word_to_pdf_enabled": true,
//!   "libreoffice_path": null,
//!   "delete_word_after_convert": false,
//!   "office_ingestion_mode": "printer_preferred",
//!   "office_auto_print_silent": true,
//!   "office_print_prompt_fallback": true,
//!   "office_intercept_extra_dirs": [],
//!   "office_debug_log_all_detected": false,
//!   "office_winword_send_prompt": false
//! }
//! ```
//! Default folders (if they exist): user Downloads/Desktop/Documents/OneDrive, CCC-related
//! `ProgramData` paths, classic `C:\\CCC\\WORKFILES`, `C:\\CCC` and `%LOCALAPPDATA%\\Temp\\CCC` when present,
//! plus `extra_dirs` from config.
//!
//! CCC ONE export locations are **per-shop** (Configure → Machine Settings → File Export / Directories).
//! If PDFs land in a subfolder, add that path to `extra_dirs` — scanning is **non-recursive** (top-level
//! PDFs in each listed folder only).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::Manager;

use crate::config::print_config;

fn default_word_to_pdf_enabled() -> bool {
    true
}

fn default_office_auto_print_silent() -> bool {
    true
}

fn default_office_print_prompt_fallback() -> bool {
    true
}

/// How Office documents should reach FileWisely: print to **FileWisely Printer** (proven path) vs LibreOffice staging.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfficeIngestionMode {
    #[default]
    PrinterPreferred,
    StagingConvert,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PdfWatchConfig {
    /// Additional directories to scan (absolute paths; created by shop IT or installer).
    #[serde(default)]
    pub extra_dirs: Vec<String>,
    /// Ignore PDFs at or below this size in bytes (default 64). Use 0 to only skip empty files.
    #[serde(default)]
    pub min_pdf_bytes: Option<u64>,
    /// When true, `.doc` / `.docx` in each watch folder (top-level) are converted to PDF via LibreOffice headless.
    #[serde(default = "default_word_to_pdf_enabled")]
    pub word_to_pdf_enabled: bool,
    /// Full path to `soffice.exe` if not in default install location. Also: env `LIBREOFFICE_SOFFICE`.
    #[serde(default)]
    pub libreoffice_path: Option<String>,
    /// Remove the Word file after a successful conversion (shop policy).
    #[serde(default)]
    pub delete_word_after_convert: bool,
    /// Prefer routing Office docs through **FileWisely Printer** (Word COM); fall back to LibreOffice when needed.
    #[serde(default)]
    pub office_ingestion_mode: OfficeIngestionMode,
    /// When `printer_preferred`, attempt silent headless Word print without a dialog.
    #[serde(default = "default_office_auto_print_silent")]
    pub office_auto_print_silent: bool,
    /// After a failed silent print, emit `uce-office-print-prompt` so the user can one-click retry.
    #[serde(default = "default_office_print_prompt_fallback")]
    pub office_print_prompt_fallback: bool,
    /// Extra directories to **recursively** watch for `.doc` / `.docx` / `.rtf` (in addition to defaults).
    #[serde(default)]
    pub office_intercept_extra_dirs: Vec<String>,
    /// Log every Office extension seen under watch roots (also set env `UCE_DEBUG_OFFICE_SOURCES=1`).
    #[serde(default)]
    pub office_debug_log_all_detected: bool,
    /// When true, foreground WINWORD with a resolvable path in the title emits `uce-filewisely-send-doc-prompt`.
    #[serde(default)]
    pub office_winword_send_prompt: bool,
}

impl Default for PdfWatchConfig {
    fn default() -> Self {
        Self {
            extra_dirs: Vec::new(),
            min_pdf_bytes: None,
            word_to_pdf_enabled: true,
            libreoffice_path: None,
            delete_word_after_convert: false,
            office_ingestion_mode: OfficeIngestionMode::default(),
            office_auto_print_silent: default_office_auto_print_silent(),
            office_print_prompt_fallback: default_office_print_prompt_fallback(),
            office_intercept_extra_dirs: Vec::new(),
            office_debug_log_all_detected: false,
            office_winword_send_prompt: false,
        }
    }
}

fn config_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir error: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create app data dir error: {e}"))?;
    Ok(dir.join("uce-pdf-watch.json"))
}

pub fn load_pdf_watch_config(app: &tauri::AppHandle) -> PdfWatchConfig {
    let path = match config_path(app) {
        Ok(p) => p,
        Err(_) => return PdfWatchConfig::default(),
    };
    if !path.exists() {
        return PdfWatchConfig::default();
    }
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => return PdfWatchConfig::default(),
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_pdf_watch_config(app: &tauri::AppHandle, cfg: &PdfWatchConfig) -> Result<(), String> {
    let path = config_path(app)?;
    let raw = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| e.to_string())
}

fn push_if_exists(out: &mut Vec<PathBuf>, p: PathBuf) {
    if p.as_os_str().is_empty() {
        return;
    }
    if Path::new(&p).exists() {
        out.push(p);
    }
}

fn paths_canon_equal(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()),
    }
}

fn path_is_under_or_equal_to(child: &Path, ancestor: &Path) -> bool {
    match (fs::canonicalize(child), fs::canonicalize(ancestor)) {
        (Ok(c), Ok(a)) => c.starts_with(&a),
        _ => {
            let cs = child.to_string_lossy().to_lowercase();
            let ax = ancestor.to_string_lossy().to_lowercase();
            cs == ax || cs.starts_with(&format!("{}\\", ax.trim_end_matches('\\')))
        }
    }
}

fn infer_office_root_label(dir: &Path) -> &'static str {
    let s = dir.to_string_lossy().to_lowercase();
    if s.contains("downloads") {
        return "downloads";
    }
    if s.contains("onedrive") {
        return "onedrive";
    }
    if s.contains("desktop") {
        return "desktop";
    }
    if s.contains("documents") && !s.contains("onedrive") {
        return "documents";
    }
    if s.contains("workfiles") {
        return "ccc_workfiles";
    }
    if s.contains("programdata") && s.contains("ccc") {
        return "ccc_programdata";
    }
    if s.contains("filewisely") {
        return "filewisely_tree";
    }
    if s.contains("ccc") {
        return "ccc_path";
    }
    "pdf_watch_candidate"
}

fn dedupe_office_watch_roots(mut v: Vec<(PathBuf, &'static str)>) -> Vec<(PathBuf, &'static str)> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (p, rule) in v.drain(..) {
        let key = fs::canonicalize(&p)
            .unwrap_or(p.clone())
            .to_string_lossy()
            .to_lowercase();
        if seen.insert(key) {
            out.push((p, rule));
        }
    }
    out
}

/// Recursive watch roots for Office interception (primary ingestion folder + likely Word output paths).
pub fn office_intercept_watch_roots(app: &tauri::AppHandle) -> Vec<(PathBuf, &'static str)> {
    let primary = print_config::watched_incoming_root();
    let primary_rule: &'static str = if print_config::ccc_temp_watch_only() {
        "ccc_temp"
    } else {
        "filewisely_incoming"
    };
    let mut out = vec![(primary.clone(), primary_rule)];

    if print_config::ccc_temp_watch_only() {
        return dedupe_office_watch_roots(out);
    }

    for dir in candidate_pdf_dirs(app) {
        if paths_canon_equal(&dir, &primary) {
            continue;
        }
        if path_is_under_or_equal_to(&dir, &primary) {
            continue;
        }
        let label = infer_office_root_label(&dir);
        out.push((dir, label));
    }

    let cfg = load_pdf_watch_config(app);
    for s in &cfg.office_intercept_extra_dirs {
        let p = PathBuf::from(s.trim());
        if !p.as_os_str().is_empty() {
            out.push((p, "office_intercept_extra"));
        }
    }

    dedupe_office_watch_roots(out)
}

/// Longest-prefix match of `path` against watch roots (for `OFFICE_SOURCE_MATCHED_RULE`).
pub fn resolve_office_source_rule(path: &Path, roots: &[(PathBuf, &'static str)]) -> &'static str {
    let path_s = path.to_string_lossy().to_lowercase();
    let mut best: &'static str = "unknown";
    let mut best_len = 0usize;
    for (root, rule) in roots {
        let r = root.to_string_lossy().to_lowercase();
        let rtrim = r.trim_end_matches('\\');
        if path_s.starts_with(rtrim) && rtrim.len() > best_len {
            best_len = rtrim.len();
            best = *rule;
        }
    }
    best
}

/// Default watch list: Downloads, Desktop, Documents, common OneDrive paths, plus `extra_dirs` from config.
pub fn candidate_pdf_dirs(app: &tauri::AppHandle) -> Vec<PathBuf> {
    if print_config::ccc_temp_watch_only() {
        let ccc = print_config::ccc_temp_watch_path();
        return dedupe_dirs(vec![ccc.clone(), ccc.join(".uce_staging")]);
    }

    let cfg = load_pdf_watch_config(app);
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        let base = PathBuf::from(&user_profile);
        push_if_exists(&mut dirs, base.join("Downloads"));
        push_if_exists(&mut dirs, base.join("Desktop"));
        push_if_exists(&mut dirs, base.join("Documents"));
        push_if_exists(&mut dirs, base.join("OneDrive").join("Desktop"));
        push_if_exists(&mut dirs, base.join("OneDrive").join("Documents"));
    }

    // CCC ONE / CCCIS — common local roots (shop may use network drives or custom export dirs instead).
    if let Ok(pd) = std::env::var("PROGRAMDATA") {
        let cccis = PathBuf::from(pd).join("CCCInformation Services");
        push_if_exists(&mut dirs, cccis.clone());
        push_if_exists(&mut dirs, cccis.join("CCCONE"));
    }
    push_if_exists(&mut dirs, PathBuf::from(r"C:\CCC\WORKFILES"));
    // CCC local export root (e.g. Change Request PDFs).
    push_if_exists(&mut dirs, PathBuf::from(r"C:\CCC"));
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        push_if_exists(&mut dirs, PathBuf::from(la).join("Temp").join("CCC"));
    }

    // FileWisely / shop ingestion (create folder or add to extra_dirs if elsewhere).
    #[cfg(windows)]
    {
        push_if_exists(&mut dirs, PathBuf::from(r"C:\FileWisely\Incoming"));
    }

    // Shop-configured paths: keep even if missing so watching starts when the folder is created later.
    for s in &cfg.extra_dirs {
        let p = PathBuf::from(s.trim());
        if !p.as_os_str().is_empty() {
            dirs.push(p);
        }
    }

    dedupe_dirs(dirs)
}

fn dedupe_dirs(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for d in dirs {
        let key = d.to_string_lossy().to_lowercase();
        if seen.insert(key) {
            out.push(d);
        }
    }
    out
}

pub fn min_pdf_bytes(cfg: &PdfWatchConfig) -> u64 {
    // Default ~64B: skip empty/1-byte junk; many CCC/print PDFs are well under 10KB and were skipped.
    cfg.min_pdf_bytes.unwrap_or(64)
}
