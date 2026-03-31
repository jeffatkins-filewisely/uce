//! CCC temp batch capture: burst mode, sweeper, queue, dedupe, and diagnostic logging.
//! Active only when [`crate::config::print_config::ccc_temp_watch_only`] is true.

use crate::config::print_config;
use crate::pdf_watch_config;
use crate::services::converter;
use crate::services::foreground_telemetry;
use crate::services::incoming_emit;
use crate::services::office_printer_route;
use serde_json::json;
use tauri::Emitter;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};
fn wait_for_pdf_file_stable(path: &Path) -> bool {
    const MAX_WAIT: usize = 40;
    const MS: u64 = 200;
    let mut last: Option<u64> = None;
    let mut stable = 0u32;
    for _ in 0..MAX_WAIT {
        if path.exists() && path.is_file() {
            if let Ok(m) = fs::metadata(path) {
                let len = m.len();
                if len >= 1 {
                    if Some(len) == last {
                        stable += 1;
                        if stable >= 3 {
                            return true;
                        }
                    } else {
                        stable = 0;
                    }
                    last = Some(len);
                }
            }
        }
        thread::sleep(Duration::from_millis(MS));
    }
    false
}

/// Dedupe key for CCC temp: full path + size + mtime (nanos). Basename-only + millis collided on rapid prints.
fn file_fingerprint(path: &Path) -> Option<String> {
    let m = fs::metadata(path).ok()?;
    let path_key = fs::canonicalize(path)
        .map(|p| p.to_string_lossy().to_lowercase())
        .unwrap_or_else(|_| path.to_string_lossy().to_lowercase());
    let mod_ns = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())?
        .as_nanos();
    Some(format!("{}|{}|{}", path_key, m.len(), mod_ns))
}

fn basename_key(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_lowercase()
}

struct RecentDedupe {
    entries: VecDeque<(String, Instant)>,
}

impl RecentDedupe {
    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    fn prune(&mut self, now: Instant) {
        while self
            .entries
            .front()
            .map(|(_, t)| now.duration_since(*t) > Duration::from_secs(120))
            .unwrap_or(false)
        {
            self.entries.pop_front();
        }
    }

    fn is_dup(&mut self, fp: &str, now: Instant) -> bool {
        self.prune(now);
        self.entries.iter().any(|(k, _)| k == fp)
    }

    fn insert(&mut self, fp: String, now: Instant) {
        self.prune(now);
        self.entries.push_back((fp, now));
    }
}

struct BatchState {
    detection_window: VecDeque<Instant>,
    burst_until: Option<Instant>,
    debug_snap_until: Option<Instant>,
    debug_snap_spawned_for_generation: bool,
    counters: BatchCounters,
    dedupe: RecentDedupe,
}

struct BatchCounters {
    seen_basenames: HashSet<String>,
    moved_basenames: HashSet<String>,
    processed_basenames: HashSet<String>,
    uploaded_basenames: HashSet<String>,
    reason_counts: HashMap<String, u32>,
}

impl BatchCounters {
    fn new() -> Self {
        Self {
            seen_basenames: HashSet::new(),
            moved_basenames: HashSet::new(),
            processed_basenames: HashSet::new(),
            uploaded_basenames: HashSet::new(),
            reason_counts: HashMap::new(),
        }
    }

    fn bump_reason(&mut self, r: &str) {
        *self.reason_counts.entry(r.to_string()).or_insert(0) += 1;
    }
}

static BATCH: Mutex<Option<BatchState>> = Mutex::new(None);

fn with_batch<T>(f: impl FnOnce(&mut BatchState) -> T) -> Option<T> {
    let mut g = BATCH.lock().ok()?;
    let st = g.get_or_insert_with(|| BatchState {
        detection_window: VecDeque::new(),
        burst_until: None,
        debug_snap_until: None,
        debug_snap_spawned_for_generation: false,
        counters: BatchCounters::new(),
        dedupe: RecentDedupe::new(),
    });
    Some(f(st))
}

pub fn init_ccc_batch_subsystems(app: &tauri::AppHandle) {
    if !print_config::ccc_temp_watch_only() {
        return;
    }
    static ONCE: OnceLock<()> = OnceLock::new();
    if ONCE.set(()).is_err() {
        return;
    }
    spawn_job_worker(app.clone());
    spawn_burst_sweeper(app.clone());
    spawn_burst_end_coordinator();
    #[cfg(windows)]
    super::ccc_cr_word_autoclose::spawn_ccc_cr_poll();
}

enum QueuedKind {
    Pdf,
    Office {
        pdf_stem: String,
        soffice: PathBuf,
    },
}

struct QueuedJob {
    staged: PathBuf,
    out_dir: PathBuf,
    basename: String,
    kind: QueuedKind,
}

static CCC_JOB_TX: OnceLock<Sender<QueuedJob>> = OnceLock::new();

fn spawn_job_worker(app: tauri::AppHandle) {
    let (tx, rx) = mpsc::channel::<QueuedJob>();
    let _ = CCC_JOB_TX.set(tx);
    thread::spawn(move || {
        while let Ok(job) = rx.recv() {
            process_queued_job(&app, job);
        }
    });
}

fn job_sender() -> Option<&'static Sender<QueuedJob>> {
    CCC_JOB_TX.get()
}

fn process_queued_job(app: &tauri::AppHandle, job: QueuedJob) {
    match job.kind {
        QueuedKind::Pdf => {
            eprintln!(
                "[UCE][ccc] processing_started kind=pdf staged={}",
                job.staged.display()
            );
            if !wait_for_pdf_file_stable(&job.staged) {
                eprintln!(
                    "[UCE][ccc] processing_finished kind=pdf result=abandoned reason=timeout_or_unstable path={}",
                    job.staged.display()
                );
                note_miss_reason("staged_pdf_unstable");
                return;
            }
            eprintln!(
                "[UCE][ccc] processing_finished kind=pdf result=ok path={}",
                job.staged.display()
            );
            let path_str = job.staged.to_string_lossy().to_string();
            incoming_emit::emit_uce_incoming_pdf(app, path_str);
            note_processed(&job.basename);
        }
        QueuedKind::Office {
            pdf_stem,
            soffice,
        } => {
            eprintln!(
                "[UCE][ccc] processing_started kind=office staged={}",
                job.staged.display()
            );
            match converter::convert_staged_office_in_queue(&soffice, &job.staged, &job.out_dir, &pdf_stem)
            {
                Ok(pdf) => {
                    eprintln!(
                        "[UCE][ccc] processing_finished kind=office result=ok pdf={}",
                        pdf.display()
                    );
                    eprintln!(
                        "[UCE] OFFICE_INGESTION_MODE=staging_convert success pdf={}",
                        pdf.display()
                    );
                    let path_str = pdf.to_string_lossy().to_string();
                    incoming_emit::emit_uce_incoming_pdf(app, path_str);
                    note_processed(&job.basename);
                }
                Err(e) => {
                    eprintln!(
                        "[UCE][ccc] processing_finished kind=office result=err err={} staged={}",
                        e,
                        job.staged.display()
                    );
                    note_miss_reason("processing_error");
                }
            }
        }
    }
}

fn enqueue_job(job: QueuedJob) -> Result<(), &'static str> {
    let tx = job_sender().ok_or("ccc job channel not ready")?;
    tx.send(job).map_err(|_| "ccc job send failed")?;
    Ok(())
}

fn note_processed(basename: &str) {
    let _ = with_batch(|st| {
        st.counters.processed_basenames.insert(basename.to_string());
    });
}

fn note_miss_reason(reason: &'static str) {
    let _ = with_batch(|st| {
        st.counters.bump_reason(reason);
    });
}

/// Called from JS upload lifecycle when CCC mode is on.
pub fn note_upload_for_batch(path: &str, success: bool) {
    if !print_config::ccc_temp_watch_only() {
        return;
    }
    let base = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_lowercase();
    let _ = with_batch(|st| {
        if success {
            st.counters.uploaded_basenames.insert(base);
        } else {
            st.counters.bump_reason("upload_failed");
        }
    });
}

fn on_eligible_file_detected(path: &Path) {
    let now = Instant::now();
    let _ = with_batch(|st| {
        let b = basename_key(path);
        st.counters.seen_basenames.insert(b.clone());

        st.detection_window.push_back(now);
        while st
            .detection_window
            .front()
            .map(|t| now.duration_since(*t) > Duration::from_secs(3))
            .unwrap_or(false)
        {
            st.detection_window.pop_front();
        }

        if st.detection_window.len() >= 2 {
            st.burst_until = Some(now + Duration::from_secs(8));
        }
        if st.burst_until.map(|u| u > now).unwrap_or(false) {
            st.burst_until = Some(now + Duration::from_secs(8));
        }

        if print_config::uce_debug_burst() {
            if st.debug_snap_until.is_none() {
                st.debug_snap_until = Some(now + Duration::from_secs(10));
                st.debug_snap_spawned_for_generation = false;
            }
            if !st.debug_snap_spawned_for_generation {
                st.debug_snap_spawned_for_generation = true;
                let root = print_config::watched_incoming_root();
                thread::spawn(move || {
                    let end = Instant::now() + Duration::from_secs(10);
                    while Instant::now() < end {
                        log_directory_snapshot(&root, "burst_debug");
                        thread::sleep(Duration::from_millis(250));
                    }
                });
            }
        }
    });
}

fn log_directory_snapshot(root: &Path, tag: &str) {
    let Ok(entries) = fs::read_dir(root) else {
        eprintln!("[UCE][ccc][dir_snapshot tag={tag}] read_dir failed: {}", root.display());
        return;
    };
    let mut lines: Vec<String> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("?");
        let (sz, mt) = match fs::metadata(&p) {
            Ok(m) => {
                let ms = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                (m.len(), ms)
            }
            Err(_) => (0, 0),
        };
        lines.push(format!("{name} size={sz} modified_unix_ms={mt}"));
    }
    lines.sort();
    eprintln!(
        "[UCE][ccc][dir_snapshot tag={}] path={} entries={} :: {}",
        tag,
        root.display(),
        lines.len(),
        lines.join(" | ")
    );
}

fn burst_active() -> bool {
    with_batch(|st| {
        let now = Instant::now();
        st.burst_until
            .map(|u| u > now)
            .unwrap_or(false)
    })
    .unwrap_or(false)
}

/// Polls `%LOCALAPPDATA%\Temp\CCC` on a fixed interval. The fs watcher can miss creates; previously we only
/// swept during an 8s burst window, leaving files stranded in temp until a rare JS poll.
fn spawn_burst_sweeper(app: tauri::AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(500));
        if !print_config::ccc_temp_watch_only() {
            continue;
        }
        let tag = if burst_active() || foreground_telemetry::ccc_printing_title_active() {
            "burst_sweeper"
        } else {
            "interval_sweeper"
        };
        sweep_ccc_temp_top_level(&app, tag);
    });
}

fn spawn_burst_end_coordinator() {
    thread::spawn(|| loop {
        thread::sleep(Duration::from_millis(100));
        if !print_config::ccc_temp_watch_only() {
            continue;
        }
        let mut should_summary = false;
        let _ = with_batch(|st| {
            let now = Instant::now();
            if let Some(until) = st.burst_until {
                if until <= now && !st.counters.seen_basenames.is_empty() {
                    should_summary = true;
                }
            }
        });
        if should_summary {
            log_batch_summary_and_reset();
        }
    });
}

fn log_batch_summary_and_reset() {
    let line = with_batch(|st| {
        let seen = st.counters.seen_basenames.len();
        let moved = st.counters.moved_basenames.len();
        let processed = st.counters.processed_basenames.len();
        let uploaded = st.counters.uploaded_basenames.len();
        let missed = seen.saturating_sub(uploaded);
        let mut parts: Vec<String> = st
            .counters
            .reason_counts
            .iter()
            .map(|(k, v)| format!("{}:{}", k, v))
            .collect();
        parts.sort();
        let reasons = if parts.is_empty() {
            String::new()
        } else {
            parts.join(",")
        };
        format!(
            "[UCE][batch_summary] seen={seen} moved={moved} processed={processed} uploaded={uploaded} missed={missed} reasons={reasons}"
        )
    });
    if let Some(l) = line {
        eprintln!("{l}");
    }
    let _ = with_batch(|st| {
        *st = BatchState {
            detection_window: VecDeque::new(),
            burst_until: None,
            debug_snap_until: None,
            debug_snap_spawned_for_generation: false,
            counters: BatchCounters::new(),
            dedupe: RecentDedupe::new(),
        };
    });
}

/// Walk `watched_incoming_root()` recursively. CCC often writes under subfolders; top-level-only sweeps
/// left PDFs stranded when notify missed or debounce coalesced. Skip `.uce_staging` trees entirely.
fn sweep_ccc_temp_top_level(app: &tauri::AppHandle, tag: &str) {
    let root = print_config::watched_incoming_root();
    if !root.exists() {
        return;
    }
    sweep_ccc_temp_visit_dir(app, root.as_path(), tag);
}

fn sweep_ccc_temp_visit_dir(app: &tauri::AppHandle, dir: &Path, tag: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    for path in children {
        if path.is_dir() {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.eq_ignore_ascii_case(".uce_staging") {
                continue;
            }
            sweep_ccc_temp_visit_dir(app, path.as_path(), tag);
        } else if path.is_file() {
            if converter::path_is_under_uce_staging(&path) {
                continue;
            }
            handle_ccc_temp_file_inner(app, path, tag);
        }
    }
}

/// Watcher entry: full CCC pipeline for one path.
pub fn handle_ccc_temp_file(app: &tauri::AppHandle, path: PathBuf) {
    handle_ccc_temp_file_inner(app, path, "fs_watcher");
}

fn handle_ccc_temp_file_inner(app: &tauri::AppHandle, path: PathBuf, source_tag: &str) {
    if !print_config::ccc_temp_watch_only() {
        return;
    }

    let meta = match fs::metadata(&path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "[UCE][ccc] detected path={} err=metadata source={} detail={}",
                path.display(),
                source_tag,
                e
            );
            return;
        }
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let initial_size = meta.len();
    eprintln!(
        "[UCE][ccc] lifecycle detected_in_watched_folder path={} ext={} initial_size={} source={}",
        path.display(),
        ext,
        initial_size,
        source_tag
    );

    let eligible = match ext.as_str() {
        "pdf" => true,
        "doc" | "docx" | "rtf" => true,
        _ => false,
    };
    if !eligible {
        eprintln!(
            "[UCE][ccc] claim_skipped reason=unsupported_extension path={} ext={}",
            path.display(),
            ext
        );
        note_miss_reason("unsupported_extension");
        return;
    }

    if ext != "pdf" && initial_size < 64 {
        eprintln!(
            "[UCE][ccc] claim_skipped reason=too_small path={} size={}",
            path.display(),
            initial_size
        );
        return;
    }

    on_eligible_file_detected(&path);

    if matches!(ext.as_str(), "doc" | "docx" | "rtf") {
        eprintln!(
            "[UCE] OFFICE_SOURCE_DETECTED OFFICE_SOURCE_PATH={} OFFICE_SOURCE_EXT={} OFFICE_SOURCE_MATCHED_RULE=ccc_temp",
            path.display(),
            ext
        );
    }

    let fp = match file_fingerprint(&path) {
        Some(f) => f,
        None => {
            eprintln!(
                "[UCE][ccc] claim_skipped reason=metadata path={}",
                path.display()
            );
            return;
        }
    };

    let now = Instant::now();
    let dup = with_batch(|st| st.dedupe.is_dup(&fp, now)).unwrap_or(false);
    if dup {
        eprintln!(
            "[UCE][ccc] claim_skipped reason=duplicate_suppressed fp={} path={}",
            fp,
            path.display()
        );
        note_miss_reason("duplicate_suppressed");
        return;
    }

    let basename = basename_key(&path);
    let out_dir = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| print_config::watched_incoming_root());
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");

    eprintln!(
        "[UCE][ccc] claim_attempt_started path={} tag={}",
        path.display(),
        source_tag
    );

    let t0 = Instant::now();

    if ext == "pdf" {
        let pdf_path_for_cr = path.clone();
        match converter::claim_pdf_for_ccc_queue(&path, &out_dir, stem, Some(t0), source_tag) {
            Ok(staged) => {
                #[cfg(windows)]
                super::ccc_cr_word_autoclose::notify_ccc_temp_pdf_claimed(&pdf_path_for_cr);
                eprintln!(
                    "[UCE][ccc] move_to_staging result=success staged={}",
                    staged.display()
                );
                let _ = with_batch(|st| {
                    st.counters.moved_basenames.insert(basename.clone());
                    st.dedupe.insert(fp, now);
                });
                let job = QueuedJob {
                    staged,
                    out_dir,
                    basename,
                    kind: QueuedKind::Pdf,
                };
                if let Err(e) = enqueue_job(job) {
                    eprintln!("[UCE][ccc] enqueue_failed kind=pdf err={e}");
                    note_miss_reason("processing_error");
                }
            }
            Err(e) if e == converter::DUPLICATE_OFFICE_PIPELINE_SKIPPED => {
                eprintln!(
                    "[UCE][ccc] move_to_staging result=skipped reason=duplicate_suppressed path={}",
                    path.display()
                );
                note_miss_reason("duplicate_suppressed");
            }
            Err(e) => {
                let reason = if !path.exists() {
                    "file_disappeared_before_claim"
                } else if e.contains("Could not claim") {
                    "move_failed_locked"
                } else {
                    "processing_error"
                };
                eprintln!(
                    "[UCE][ccc] move_to_staging result=failure path={} err={} probable_cause={}",
                    path.display(),
                    e,
                    reason
                );
                note_miss_reason(reason);
            }
        }
        return;
    }

    // Office / RTF
    let cfg = pdf_watch_config::load_pdf_watch_config(app);

    match office_printer_route::route_office_document_printer_first(app, &path, &cfg, "ccc_temp") {
        office_printer_route::OfficePrinterFirstResult::SkipDuplicateWindow => return,
        office_printer_route::OfficePrinterFirstResult::HandledByPrinter => {
            let _ = with_batch(|st| {
                st.counters.moved_basenames.insert(basename.clone());
                st.dedupe.insert(fp.clone(), now);
            });
            note_processed(&basename);
            return;
        }
        office_printer_route::OfficePrinterFirstResult::FallThroughToStaging => {}
    }

    if !cfg.word_to_pdf_enabled {
        eprintln!("[UCE][ccc] office_skipped word_to_pdf_disabled path={}", path.display());
        return;
    }
    let Some(soffice) = converter::resolve_soffice_path(cfg.libreoffice_path.as_deref()) else {
        eprintln!("[UCE][ccc] office_skipped libreoffice_missing path={}", path.display());
        return;
    };

    let pdf_path = out_dir.join(format!("{stem}.pdf"));
    if !converter::needs_conversion(&path, &pdf_path) {
        eprintln!(
            "[UCE][ccc] office_skip_no_conversion path={} pdf_exists={}",
            path.display(),
            pdf_path.display()
        );
        let path_str = pdf_path.to_string_lossy().to_string();
        incoming_emit::emit_uce_incoming_pdf(app, path_str);
        return;
    }

    match converter::claim_office_for_ccc_queue(&path, &out_dir, stem, Some(t0), source_tag) {
        Ok(staged) => {
            eprintln!(
                "[UCE][ccc] move_to_staging result=success staged={}",
                staged.display()
            );
            let _ = with_batch(|st| {
                st.counters.moved_basenames.insert(basename.clone());
                st.dedupe.insert(fp, now);
            });
            let job = QueuedJob {
                staged,
                out_dir,
                basename,
                kind: QueuedKind::Office {
                    pdf_stem: stem.to_string(),
                    soffice,
                },
            };
            if let Err(e) = enqueue_job(job) {
                eprintln!("[UCE][ccc] enqueue_failed kind=office err={e}");
                note_miss_reason("processing_error");
            }
        }
        Err(e) if e == converter::DUPLICATE_OFFICE_PIPELINE_SKIPPED => {
            eprintln!(
                "[UCE][ccc] move_to_staging result=skipped reason=duplicate_suppressed path={}",
                path.display()
            );
            note_miss_reason("duplicate_suppressed");
        }
        Err(e) => {
            let reason = if !path.exists() {
                "file_disappeared_before_claim"
            } else if e.contains("Could not claim") {
                "move_failed_locked"
            } else {
                "processing_error"
            };
            eprintln!(
                "[UCE][ccc] move_to_staging result=failure path={} err={} probable_cause={}",
                path.display(),
                e,
                reason
            );
            note_miss_reason(reason);
            if reason == "move_failed_locked" && cfg.office_print_prompt_fallback && path.is_file() {
                let _ = app.emit(
                    "uce-office-print-prompt",
                    json!({
                        "path": path.to_string_lossy().to_string(),
                        "message": "Send this document to FileWisely?",
                        "reason": "claim_failed",
                    }),
                );
            }
        }
    }
}

/// JS `list_pdf_metas_since` / poll path: same sweep as burst sweeper (claims + enqueue).
pub fn poll_ccc_temp_sweep(app: &tauri::AppHandle) {
    if !print_config::ccc_temp_watch_only() {
        return;
    }
    sweep_ccc_temp_top_level(app, "poll");
}
