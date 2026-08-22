//! Extra PDF watch locations (CCC exports, DMS folders) and optional minimum file size.
//!
//! Config file (next to other UCE app data): **`uce-pdf-watch.json`**
//! ```json
//! {
//!   "general_document_capture_enabled": true,
//!   "general_min_file_bytes": 512,
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
//! `ProgramData` paths, classic `C:\\CCC\\WORKFILES`, `C:\\CCC`, **first-level subfolders** under
//! `C:\\CCC` and under `%ProgramData%\\CCCInformation Services` (skipping obvious non-export dirs),
//! `%LOCALAPPDATA%\\Temp\\CCC` (**always** listed so UCE creates it and watches even before CCC runs),
//! Windows scan destinations (`Pictures\\Scanned Documents`, Epson/Brother vendor folders,
//! `C:\\FileWisely\\Scans`), plus `extra_dirs` from config.
//!
//! **Machine seed:** `C:\\FileWisely\\App\\uce-pdf-watch.seed.json` (optional JSON with `extra_dirs` /
//! `office_intercept_extra_dirs`) is **unioned** with per-user `uce-pdf-watch.json` so elevated installers
//! can add paths without writing each user’s `%AppData%` profile.
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
    false
}

fn default_general_document_capture_enabled() -> bool {
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
    /// When true, watch user Documents / Downloads / Desktop / OneDrive for PDF and Office files (hybrid non-CCC capture).
    #[serde(default = "default_general_document_capture_enabled")]
    pub general_document_capture_enabled: bool,
    /// Minimum file size in bytes for files under general paths (default 512). PDFs also use `min_pdf_bytes` as a floor.
    #[serde(default)]
    pub general_min_file_bytes: Option<u64>,
    /// Machine-learned CCC export/temp folders from [`crate::services::ccc_autodiscovery`].
    #[serde(default)]
    pub auto_discovered_ccc_dirs: Vec<String>,
    /// Machine-learned print/scan destination folders from [`crate::services::source_autodiscovery`].
    #[serde(default)]
    pub auto_discovered_source_dirs: Vec<String>,
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
            general_document_capture_enabled: default_general_document_capture_enabled(),
            general_min_file_bytes: None,
            auto_discovered_ccc_dirs: Vec::new(),
            auto_discovered_source_dirs: Vec::new(),
        }
    }
}

fn append_auto_discovered_ccc_watch_roots(
    cfg: &PdfWatchConfig,
    out: &mut Vec<(PathBuf, &'static str)>,
) {
    for s in &cfg.auto_discovered_ccc_dirs {
        let p = PathBuf::from(s.trim());
        if !p.as_os_str().is_empty() {
            out.push((p, "ccc_autodiscovered"));
        }
    }
}

fn append_auto_discovered_source_watch_roots(
    cfg: &PdfWatchConfig,
    out: &mut Vec<(PathBuf, &'static str)>,
) {
    for s in &cfg.auto_discovered_source_dirs {
        let p = PathBuf::from(s.trim());
        if !p.as_os_str().is_empty() {
            out.push((p, "source_autodiscovered"));
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

/// Ensures `uce-pdf-watch.json` exists with defaults so support can verify watch settings on disk.
/// Returns `Ok(true)` when the file was created.
pub fn ensure_default_pdf_watch_config_file(app: &tauri::AppHandle) -> Result<bool, String> {
    let path = config_path(app)?;
    if path.exists() {
        return Ok(false);
    }
    let cfg = PdfWatchConfig::default();
    save_pdf_watch_config(app, &cfg)?;
    eprintln!(
        "UCE_PDF_WATCH_CONFIG_CREATED path={}",
        path.display()
    );
    Ok(true)
}

/// Partial document merged at runtime with per-user [`PdfWatchConfig`] (see module docs).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct PdfWatchSeed {
    #[serde(default)]
    extra_dirs: Vec<String>,
    #[serde(default)]
    office_intercept_extra_dirs: Vec<String>,
}

fn load_pdf_watch_seed() -> PdfWatchSeed {
    let path = print_config::filewisely_pdf_watch_seed_path();
    if !path.exists() {
        return PdfWatchSeed::default();
    }
    let raw = match fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => return PdfWatchSeed::default(),
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn push_if_exists(out: &mut Vec<PathBuf>, p: PathBuf) {
    if p.as_os_str().is_empty() {
        return;
    }
    if Path::new(&p).exists() {
        out.push(p);
    }
}

/// Immediate child directories of `parent` (non-recursive). Skips names in `skip_lowercase_names`
/// (compared case-insensitively) to avoid watching logs/temp under ProgramData.
fn push_first_level_subdirs(out: &mut Vec<PathBuf>, parent: &Path, skip_lowercase_names: &[&str]) {
    if !parent.exists() {
        return;
    }
    let skip: HashSet<String> = skip_lowercase_names
        .iter()
        .map(|s| (*s).to_lowercase())
        .collect();
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if skip.contains(&name.to_lowercase()) {
            continue;
        }
        out.push(path);
    }
}

pub fn paths_canon_equal(a: &Path, b: &Path) -> bool {
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
    if s.contains("filewisely") && s.contains("scans") {
        return "scan_filewisely";
    }
    if s.contains("filewisely") {
        return "filewisely_tree";
    }
    if s.contains("scanned documents") || s.contains("\\fax") || s.contains("/fax") {
        return "scan_documents";
    }
    if s.contains("epson") || s.contains("brother") || s.contains("canon") || s.contains("twain") {
        return "scan_vendor";
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
        let cfg = load_pdf_watch_config(app);
        append_auto_discovered_ccc_watch_roots(&cfg, &mut out);
        append_auto_discovered_source_watch_roots(&cfg, &mut out);
        return dedupe_office_watch_roots(out);
    }

    let cfg = load_pdf_watch_config(app);
    let seed = load_pdf_watch_seed();

    for dir in ccc_core_candidate_dirs(app) {
        if paths_canon_equal(&dir, &primary) {
            continue;
        }
        if path_is_under_or_equal_to(&dir, &primary) {
            continue;
        }
        let label = infer_office_root_label(&dir);
        out.push((dir, label));
    }

    if cfg.general_document_capture_enabled {
        for (dir, rule) in general_user_document_roots_with_rules() {
            if paths_canon_equal(&dir, &primary) {
                continue;
            }
            if path_is_under_or_equal_to(&dir, &primary) {
                continue;
            }
            out.push((dir, rule));
        }
    }

    for s in cfg
        .office_intercept_extra_dirs
        .iter()
        .chain(seed.office_intercept_extra_dirs.iter())
    {
        let p = PathBuf::from(s.trim());
        if !p.as_os_str().is_empty() {
            out.push((p, "office_intercept_extra"));
        }
    }

    append_auto_discovered_ccc_watch_roots(&cfg, &mut out);
    append_auto_discovered_source_watch_roots(&cfg, &mut out);

    for (dir, rule) in scan_destination_roots_with_rules() {
        if paths_canon_equal(&dir, &primary) {
            continue;
        }
        out.push((dir, rule));
    }

    dedupe_office_watch_roots(out)
}

/// CCC-first roots: ProgramData CCCIS, `C:\CCC`, `%LOCALAPPDATA%\Temp\CCC`, FileWisely Incoming, `extra_dirs`.
pub fn ccc_core_candidate_dirs(app: &tauri::AppHandle) -> Vec<PathBuf> {
    let cfg = load_pdf_watch_config(app);
    let seed = load_pdf_watch_seed();
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Ok(pd) = std::env::var("PROGRAMDATA") {
        let cccis = PathBuf::from(pd).join("CCCInformation Services");
        push_if_exists(&mut dirs, cccis.clone());
        push_if_exists(&mut dirs, cccis.join("CCCONE"));
        push_first_level_subdirs(
            &mut dirs,
            &cccis,
            &["logs", "log", "temp", "tmp", "cache", "installer"],
        );
    }
    push_if_exists(&mut dirs, PathBuf::from(r"C:\CCC\WORKFILES"));
    push_if_exists(&mut dirs, PathBuf::from(r"C:\CCC"));
    push_first_level_subdirs(&mut dirs, Path::new(r"C:\CCC"), &[]);
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(la).join("Temp").join("CCC"));
    }

    #[cfg(windows)]
    {
        push_if_exists(&mut dirs, PathBuf::from(r"C:\FileWisely\Incoming"));
        dirs.push(PathBuf::from(print_config::FW_SCANS_DIR));
        for (dir, _) in scan_destination_roots_with_rules() {
            dirs.push(dir);
        }
    }

    for s in cfg.extra_dirs.iter().chain(seed.extra_dirs.iter()) {
        let p = PathBuf::from(s.trim());
        if !p.as_os_str().is_empty() {
            dirs.push(p);
        }
    }

    dedupe_dirs(dirs)
}

fn push_general_pair(out: &mut Vec<(PathBuf, &'static str)>, dir: PathBuf, rule: &'static str) {
    if dir.as_os_str().is_empty() {
        return;
    }
    if dir.exists() {
        out.push((dir, rule));
    }
}

/// Secondary hybrid capture: standard profile folders with stable `general_*` rule ids.
pub fn general_user_document_roots_with_rules() -> Vec<(PathBuf, &'static str)> {
    let mut out = Vec::new();
    let Ok(user_profile) = std::env::var("USERPROFILE") else {
        return out;
    };
    let base = PathBuf::from(&user_profile);
    push_general_pair(&mut out, base.join("Downloads"), "general_downloads");
    push_general_pair(&mut out, base.join("Desktop"), "general_desktop");
    push_general_pair(&mut out, base.join("Documents"), "general_documents");
    push_general_pair(
        &mut out,
        base.join("OneDrive").join("Desktop"),
        "general_onedrive_desktop",
    );
    push_general_pair(
        &mut out,
        base.join("OneDrive").join("Documents"),
        "general_onedrive_documents",
    );
    out
}

pub fn general_user_document_dirs() -> Vec<PathBuf> {
    general_user_document_roots_with_rules()
        .into_iter()
        .map(|(p, _)| p)
        .collect()
}

pub fn is_general_capture_rule(rule: &str) -> bool {
    rule.starts_with("general_")
}

/// Folders that are expected to receive WIA / vendor scanner output (images + PDFs).
pub fn is_scan_source_rule(rule: &str) -> bool {
    matches!(
        rule,
        "scan_documents"
            | "scan_pictures"
            | "scan_vendor"
            | "scan_filewisely"
            | "source_autodiscovered"
    )
}

/// Default Windows / vendor scan destinations. Missing folders are skipped except `C:\FileWisely\Scans`.
pub fn scan_destination_roots_with_rules() -> Vec<(PathBuf, &'static str)> {
    let mut out = Vec::new();
    out.push((
        PathBuf::from(print_config::FW_SCANS_DIR),
        "scan_filewisely",
    ));

    let Ok(user_profile) = std::env::var("USERPROFILE") else {
        return out;
    };
    let base = PathBuf::from(&user_profile);
    let pictures = base.join("Pictures");
    let documents = base.join("Documents");
    let onedrive = base.join("OneDrive");

    let pairs: [(&Path, &str, &'static str); 14] = [
        (&pictures, "Scanned Documents", "scan_pictures"),
        (&documents, "Scanned Documents", "scan_documents"),
        (&documents, "Fax", "scan_documents"),
        (&documents, "EPSON Scan", "scan_vendor"),
        (&documents, "Epson Scan", "scan_vendor"),
        (&documents, "Epson", "scan_vendor"),
        (&documents, "Brother", "scan_vendor"),
        (&pictures, "EPSON Scan", "scan_vendor"),
        (&pictures, "Epson", "scan_vendor"),
        (&pictures, "Brother", "scan_vendor"),
        (&onedrive, "Pictures\\Scanned Documents", "scan_pictures"),
        (&onedrive, "Documents\\Scanned Documents", "scan_documents"),
        (&onedrive, "Documents\\EPSON Scan", "scan_vendor"),
        (&onedrive, "Documents\\Brother", "scan_vendor"),
    ];
    for (parent, child, rule) in pairs {
        push_general_pair(&mut out, parent.join(child), rule);
    }
    out
}

/// Minimum size for PDFs and Office files under `general_*` roots.
pub fn effective_general_min_bytes(cfg: &PdfWatchConfig) -> u64 {
    cfg.general_min_file_bytes.unwrap_or(512).max(64)
}

/// Skip Office lock files, partial downloads, and non-CCC `%TEMP%` junk (never CCC `Temp\CCC`).
pub fn should_ignore_general_document_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    if name.starts_with("~$") {
        return true;
    }
    if matches!(name.as_str(), "thumbs.db" | "desktop.ini") {
        return true;
    }
    if name.ends_with(".crdownload")
        || name.ends_with(".partial")
        || name.ends_with(".download")
    {
        return true;
    }
    let lower = path.to_string_lossy().to_lowercase();
    if lower.contains("\\appdata\\local\\temp\\") && !lower.contains("\\temp\\ccc") {
        return true;
    }
    false
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

/// Watch list: CCC-first roots plus optional general user folders when `general_document_capture_enabled`.
pub fn candidate_pdf_dirs(app: &tauri::AppHandle) -> Vec<PathBuf> {
    if print_config::ccc_temp_watch_only() {
        let ccc = print_config::ccc_temp_watch_path();
        return dedupe_dirs(vec![ccc.clone(), ccc.join(".uce_staging")]);
    }

    let cfg = load_pdf_watch_config(app);
    let mut dirs = ccc_core_candidate_dirs(app);

    if cfg.general_document_capture_enabled {
        dirs.extend(general_user_document_dirs());
    }

    for (dir, _) in scan_destination_roots_with_rules() {
        dirs.push(dir);
    }
    for s in &cfg.auto_discovered_source_dirs {
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
