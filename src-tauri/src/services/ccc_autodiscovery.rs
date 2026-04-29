//! Scan typical CCC-related trees for **recent** PDF/Office files whose names suggest CCC output,
//! then persist discovered parent folders into `uce-pdf-watch.json` → `auto_discovered_ccc_dirs`.

use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

use crate::pdf_watch_config;

#[derive(Debug, Clone, Serialize)]
pub struct CccAutodiscoverySummary {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub added: Vec<String>,
    pub candidates: Vec<String>,
    pub confidence: f64,
    pub no_new_dirs: bool,
    pub trigger: String,
    #[serde(default)]
    pub scan_visits: usize,
}

struct LastState {
    last_run_unix_ms: i64,
    candidates: Vec<String>,
    confidence: f64,
    last_added: Vec<String>,
}

static LAST: Mutex<Option<LastState>> = Mutex::new(None);

const RECENT_SECS: u64 = 24 * 3600;
const MAX_DEPTH: usize = 7;
const MAX_VISITS: usize = 6000;

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn is_recent_mtime(t: SystemTime) -> bool {
    SystemTime::now()
        .duration_since(t)
        .map(|d| d <= Duration::from_secs(RECENT_SECS))
        .unwrap_or(false)
}

fn safe_plausible_path(p: &Path) -> bool {
    let s = p.to_string_lossy().to_lowercase();
    if s.contains("\\windows\\") || s.starts_with("c:\\windows") {
        return false;
    }
    if s.contains("\\program files\\") || s.contains("\\program files (x86)\\") {
        return false;
    }
    true
}

/// Recent file whose name/extension matches CCC / estimate-like signals.
fn score_recent_candidate_file(path: &Path) -> Option<f64> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta.modified().ok()?;
    if !is_recent_mtime(mtime) {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    let lower = name.to_lowercase();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let doc_ext = matches!(ext.as_str(), "pdf" | "doc" | "docx" | "rtf");

    let keywords = [
        "ccc",
        "estimate",
        "supplement",
        "change request",
        "repair order",
        "workfile",
        "invoice",
        "claim",
        "initial",
        "record",
    ];
    let mut kw_hit = false;
    let mut score = 0.0f64;
    for kw in keywords {
        if lower.contains(kw) {
            kw_hit = true;
            score += 0.18;
        }
    }
    if lower.contains("change") && lower.contains("request") {
        kw_hit = true;
        score += 0.12;
    }
    if lower.contains("repair") && lower.contains("order") {
        kw_hit = true;
        score += 0.15;
    }
    if lower.contains("ccc") {
        score += 0.25;
    }

    if !(doc_ext || kw_hit) {
        return None;
    }
    if doc_ext {
        score += 0.2;
    }
    Some(score.min(1.0))
}

fn collect_seed_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        let temp = PathBuf::from(&la).join("Temp");
        v.push(temp.join("CCC"));
        if let Ok(rd) = fs::read_dir(&temp) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    let n = e.file_name().to_string_lossy().to_lowercase();
                    if n.contains("ccc") {
                        v.push(e.path());
                    }
                }
            }
        }
    }
    v.push(PathBuf::from(r"C:\CCC"));
    v.push(PathBuf::from(r"C:\CCC\WORKFILES"));
    v.push(PathBuf::from(r"C:\CCC\Exports"));
    if let Ok(pd) = std::env::var("PROGRAMDATA") {
        if let Ok(rd) = fs::read_dir(&pd) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    let n = e.file_name().to_string_lossy().to_lowercase();
                    if n.contains("ccc") {
                        v.push(e.path());
                    }
                }
            }
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let doc = PathBuf::from(&home).join("Documents");
        v.push(doc.join("CCC ONE"));
        v.push(doc.join("CCC"));
        for od in [
            doc.join("OneDrive").join("Documents"),
            PathBuf::from(&home).join("OneDrive").join("Documents"),
        ] {
            if let Ok(rd) = fs::read_dir(&od) {
                for e in rd.flatten() {
                    if e.path().is_dir() {
                        let n = e.file_name().to_string_lossy().to_lowercase();
                        if n.contains("ccc") {
                            v.push(e.path());
                        }
                    }
                }
            }
        }
    }
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        if let Ok(rd) = fs::read_dir(&la) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    let n = e.file_name().to_string_lossy().to_lowercase();
                    if n.contains("ccc") {
                        v.push(e.path());
                    }
                }
            }
        }
    }
    if let Ok(ad) = std::env::var("APPDATA") {
        if let Ok(rd) = fs::read_dir(&ad) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    let n = e.file_name().to_string_lossy().to_lowercase();
                    if n.contains("ccc") {
                        v.push(e.path());
                    }
                }
            }
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

struct ScanOutcome {
    folders: HashSet<PathBuf>,
    candidates: Vec<String>,
    scores: Vec<f64>,
    visits: usize,
}

fn scan_tree(root: &Path, outcome: &mut ScanOutcome, depth: usize) {
    if outcome.visits >= MAX_VISITS || depth > MAX_DEPTH {
        return;
    }
    if !root.exists() || !root.is_dir() {
        return;
    }
    if !safe_plausible_path(root) {
        return;
    }
    outcome.visits += 1;

    let Ok(rd) = fs::read_dir(root) else {
        return;
    };

    for entry in rd.flatten() {
        if outcome.visits >= MAX_VISITS {
            return;
        }
        let path = entry.path();
        if path.is_file() {
            outcome.visits += 1;
            if let Some(conf) = score_recent_candidate_file(&path) {
                if let Some(parent) = path.parent() {
                    if safe_plausible_path(parent)
                        && outcome.folders.insert(parent.to_path_buf())
                    {
                        let ps = parent.to_string_lossy().to_string();
                        outcome.candidates.push(ps.clone());
                        outcome.scores.push(conf);
                        eprintln!(
                            "UCE_CCC_AUTODISCOVERY_CANDIDATE path={} conf={:.2}",
                            ps, conf
                        );
                    }
                }
            }
        } else if path.is_dir() {
            scan_tree(&path, outcome, depth + 1);
        }
    }
}

pub fn diagnostics_snapshot() -> serde_json::Value {
    let g = LAST.lock().unwrap();
    let Some(st) = g.as_ref() else {
        return serde_json::json!({
            "ccc_autodiscovery_last_run": serde_json::Value::Null,
            "ccc_autodiscovery_candidates": [],
            "ccc_autodiscovery_confidence": 0.0,
            "ccc_autodiscovery_last_added": [],
        });
    };
    serde_json::json!({
        "ccc_autodiscovery_last_run_unix_ms": st.last_run_unix_ms,
        "ccc_autodiscovery_candidates": st.candidates,
        "ccc_autodiscovery_confidence": st.confidence,
        "ccc_autodiscovery_last_added": st.last_added,
    })
}

pub fn run_for_app(app: &AppHandle, trigger: &str) -> CccAutodiscoverySummary {
    #[cfg(not(windows))]
    {
        let _ = app;
        let _ = trigger;
        return CccAutodiscoverySummary {
            ok: false,
            error: Some("CCC autodiscovery is Windows-only".into()),
            added: vec![],
            candidates: vec![],
            confidence: 0.0,
            no_new_dirs: true,
            trigger: trigger.to_string(),
            scan_visits: 0,
        };
    }

    #[cfg(windows)]
    {
        run_for_app_inner(app, trigger)
    }
}

#[cfg(windows)]
fn run_for_app_inner(app: &AppHandle, trigger: &str) -> CccAutodiscoverySummary {
    eprintln!("UCE_CCC_AUTODISCOVERY_STARTED trigger={}", trigger);

    let seeds = collect_seed_roots();
    let mut outcome = ScanOutcome {
        folders: HashSet::new(),
        candidates: Vec::new(),
        scores: Vec::new(),
        visits: 0,
    };

    for seed in seeds {
        if seed.exists() && seed.is_dir() {
            scan_tree(&seed, &mut outcome, 0);
        }
    }

    let had_candidate_folders = !outcome.folders.is_empty();
    let discovered: Vec<PathBuf> = outcome.folders.into_iter().collect();
    let mut cfg = pdf_watch_config::load_pdf_watch_config(app);
    let before = cfg.auto_discovered_ccc_dirs.clone();
    let mut merged: Vec<String> = before.clone();

    let mut added: Vec<String> = Vec::new();
    for p in discovered {
        let s = p.to_string_lossy().to_string();
        if !merged.iter().any(|x| x.eq_ignore_ascii_case(&s.trim())) {
            merged.push(s.clone());
            added.push(s.clone());
            eprintln!("UCE_CCC_AUTODISCOVERY_ADDED path={}", s);
        }
    }

    let confidence = if outcome.scores.is_empty() {
        0.0
    } else {
        outcome.scores.iter().sum::<f64>() / outcome.scores.len() as f64
    };

    if !had_candidate_folders && added.is_empty() {
        eprintln!("UCE_CCC_AUTODISCOVERY_NO_MATCH");
    }

    let candidates_trim: Vec<String> = outcome.candidates.into_iter().take(120).collect();

    let save_needed = merged != before;
    let no_new_dirs = added.is_empty();
    if save_needed {
        cfg.auto_discovered_ccc_dirs = merged;
        if let Err(e) = pdf_watch_config::save_pdf_watch_config(app, &cfg) {
            eprintln!("UCE_CCC_AUTODISCOVERY_COMPLETE save_failed err={}", e);
            return CccAutodiscoverySummary {
                ok: false,
                error: Some(e),
                added,
                candidates: candidates_trim,
                confidence,
                no_new_dirs,
                trigger: trigger.to_string(),
                scan_visits: outcome.visits,
            };
        }
    }

    {
        let mut g = LAST.lock().unwrap();
        *g = Some(LastState {
            last_run_unix_ms: now_unix_ms(),
            candidates: candidates_trim.clone(),
            confidence,
            last_added: added.clone(),
        });
    }

    eprintln!(
        "UCE_CCC_AUTODISCOVERY_COMPLETE added_count={} visits={}",
        added.len(),
        outcome.visits
    );

    CccAutodiscoverySummary {
        ok: true,
        error: None,
        added,
        candidates: candidates_trim,
        confidence,
        no_new_dirs,
        trigger: trigger.to_string(),
        scan_visits: outcome.visits,
    }
}
