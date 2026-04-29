//! CCC capture diagnostics: recent files ring buffer and interactive PDF sweep test.

use crate::config::print_config;
use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

fn last_ccc_store() -> &'static Mutex<VecDeque<(Instant, String)>> {
    static CELL: OnceLock<Mutex<VecDeque<(Instant, String)>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(VecDeque::with_capacity(24)))
}

/// Returns true for CCC temp, `C:\CCC*`, ProgramData CCCIS trees, etc.
pub fn is_ccc_related_path(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_lowercase();
    if lower.contains("\\temp\\ccc") || lower.contains("/temp/ccc") {
        return true;
    }
    if lower.starts_with("c:\\ccc") {
        return true;
    }
    if lower.contains("cccinformation services") {
        return true;
    }
    if lower.contains("programdata") && lower.contains("\\ccc") {
        return true;
    }
    false
}

fn should_record_from_rule(matched_rule: &str) -> bool {
    matches!(
        matched_rule,
        "ccc_temp" | "ccc_path" | "ccc_workfiles" | "ccc_programdata" | "ccc_autodiscovered"
    )
}

/// Record a PDF or Office file seen under CCC-related watch roots (deduped).
pub fn record_ccc_file_seen(path: &Path, matched_rule: Option<&str>) {
    let rule_ok = matched_rule.map(should_record_from_rule).unwrap_or(false);
    if !is_ccc_related_path(path) && !rule_ok {
        return;
    }
    let s = path.to_string_lossy().to_string();
    let mut g = last_ccc_store().lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if let Some((t, last)) = g.back() {
        if last == &s && now.duration_since(*t) < Duration::from_millis(400) {
            return;
        }
    }
    g.push_back((now, s.clone()));
    while g.len() > 20 {
        g.pop_front();
    }
    eprintln!("UCE_CCC_FILE_SEEN path={}", s);
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CccCaptureTestResult {
    pub ok: bool,
    pub path: Option<String>,
    pub waited_ms: u64,
    pub message: String,
    pub roots_scanned: Vec<String>,
}

pub fn last_ccc_files_seen() -> Vec<String> {
    last_ccc_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(_, s)| s.clone())
        .collect()
}

/// Roots for the capture test: always CCC `%LOCALAPPDATA%\Temp\CCC`, plus common CCC dirs if present.
pub fn ccc_capture_test_scan_roots() -> Vec<PathBuf> {
    let mut v = vec![print_config::ccc_temp_watch_path()];
    if let Ok(pd) = std::env::var("PROGRAMDATA") {
        let cccis = PathBuf::from(pd).join("CCCInformation Services");
        if cccis.exists() {
            v.push(cccis);
        }
    }
    for p in [r"C:\CCC\WORKFILES", r"C:\CCC"] {
        let pb = PathBuf::from(p);
        if pb.exists() {
            v.push(pb);
        }
    }
    let mut seen = HashSet::new();
    v.retain(|p| seen.insert(p.to_string_lossy().to_lowercase()));
    v
}

fn collect_pdfs_recursive(root: &Path, out: &mut HashSet<PathBuf>) {
    if !root.exists() {
        return;
    }
    let Ok(read) = std::fs::read_dir(root) else {
        return;
    };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_pdfs_recursive(&p, out);
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| x.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false)
        {
            out.insert(p);
        }
    }
}

/// Poll CCC-related folders for a **new** PDF not present at test start (blocking up to `requested_secs`, clamped 5–300).
pub fn run_ccc_capture_test(requested_secs: u64) -> CccCaptureTestResult {
    let timeout_secs = requested_secs.clamp(5, 300);
    let timeout = Duration::from_secs(timeout_secs);
    let roots = ccc_capture_test_scan_roots();
    let roots_scanned: Vec<String> = roots.iter().map(|p| p.to_string_lossy().into_owned()).collect();

    for r in &roots {
        if let Err(e) = std::fs::create_dir_all(r) {
            eprintln!(
                "[UCE] CCC capture test: create_dir_all {} err={}",
                r.display(),
                e
            );
        }
    }

    let mut initial = HashSet::new();
    for r in &roots {
        collect_pdfs_recursive(r, &mut initial);
    }

    let start = Instant::now();
    eprintln!(
        "UCE_CCC_CAPTURE_TEST_START timeout_secs={} (requested={}) roots={} initial_pdf_count={}",
        timeout_secs,
        requested_secs,
        roots.len(),
        initial.len()
    );

    while start.elapsed() < timeout {
        thread::sleep(Duration::from_millis(500));
        let mut current = HashSet::new();
        for r in &roots {
            collect_pdfs_recursive(r, &mut current);
        }
        for p in current.difference(&initial) {
            if p.is_file() {
                let s = p.to_string_lossy().to_string();
                eprintln!("UCE_CCC_CAPTURE_TEST_HIT path={}", s);
                return CccCaptureTestResult {
                    ok: true,
                    path: Some(s),
                    waited_ms: start.elapsed().as_millis() as u64,
                    message: "New PDF appeared under CCC-related roots".to_string(),
                    roots_scanned,
                };
            }
        }
    }

    CccCaptureTestResult {
        ok: false,
        path: None,
        waited_ms: start.elapsed().as_millis() as u64,
        message: format!(
            "No new PDF under CCC roots within {}s (initial baseline had {} PDFs)",
            timeout_secs.max(1),
            initial.len()
        ),
        roots_scanned,
    }
}
