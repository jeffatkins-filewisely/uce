//! Headless Word/Office → PDF via LibreOffice `soffice` (Windows shop installs).
//!
//! Install: <https://www.libreoffice.org/download/> — default `C:\Program Files\LibreOffice\program\soffice.exe`
//! Override: set `LIBREOFFICE_SOFFICE` to the full path, or `libreoffice_path` in `uce-pdf-watch.json`.
//!
//! **Ingestion model:** Office files are **never** converted while they still live on the visible Incoming
//! path. [`ingest_office_incoming_to_pdf`] **claims** the file first (rename into staging), then runs
//! stability + readable checks **only on the staged path**, then LibreOffice headless. UCE does not
//! launch Word. FileWisely Incoming uses `C:\FileWisely\.uce_staging` (not `Incoming\.uce_staging`).

use crate::config::print_config;
use crate::services::foreground_telemetry;
use crate::services::pipeline_stage_diag;

use serde_json::json;
use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::fs;

static OFFICE_TO_PDF_LOCK: Mutex<()> = Mutex::new(());

/// Returned when another pipeline (e.g. watcher + poll) already owns this path.
pub const DUPLICATE_OFFICE_PIPELINE_SKIPPED: &str = "duplicate office pipeline (skipped)";

fn incoming_pipeline_locks() -> &'static Mutex<HashSet<String>> {
    static CELL: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(HashSet::new()))
}

struct IncomingPipelineGuard {
    key: String,
}

impl Drop for IncomingPipelineGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = incoming_pipeline_locks().lock() {
            g.remove(&self.key);
        }
    }
}

fn try_acquire_incoming_pipeline(key: String) -> Option<IncomingPipelineGuard> {
    let mut g = incoming_pipeline_locks().lock().ok()?;
    if g.contains(&key) {
        return None;
    }
    g.insert(key.clone());
    Some(IncomingPipelineGuard { key })
}

pub fn incoming_pipeline_key(incoming_path: &Path) -> String {
    fs::canonicalize(incoming_path)
        .unwrap_or_else(|_| incoming_path.to_path_buf())
        .to_string_lossy()
        .to_lowercase()
}

/// Staged names are `{nanos}_{original_stem}.ext` — recover the PDF/output stem.
fn pdf_stem_from_staged_filename(path: &Path) -> Result<String, String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Office file has no stem".to_string())?;
    Ok(strip_nano_stem_prefix(stem))
}

fn strip_nano_stem_prefix(stem: &str) -> String {
    let b = stem.as_bytes();
    let mut i = 0usize;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i < b.len() && b[i] == b'_' {
        stem[i + 1..].to_string()
    } else {
        stem.to_string()
    }
}

/// Wait until a `.doc` / `.docx` has a stable size (writer finished).
pub fn wait_for_office_file_stable(path: &Path) -> bool {
    const MAX_WAIT: usize = 35;
    const MS: u64 = 200;
    let mut last: Option<u64> = None;
    let mut stable = 0u32;
    for _ in 0..MAX_WAIT {
        if path.exists() && path.is_file() {
            if let Ok(m) = fs::metadata(path) {
                let len = m.len();
                if len >= 64 {
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

/// After size is stable, wait until the OS allows opening the file for read (Word may hold locks).
pub fn wait_until_file_readable(path: &Path) -> bool {
    const ROUNDS: usize = 15;
    const MS: u64 = 300;
    for _ in 0..ROUNDS {
        if path.exists() && path.is_file() {
            match std::fs::File::open(path) {
                Ok(_) => return true,
                Err(_) => thread::sleep(Duration::from_millis(MS)),
            }
        } else {
            thread::sleep(Duration::from_millis(MS));
        }
    }
    false
}

/// Size-stable → readable → size-stable again (catches writers that flush after the file becomes readable).
fn wait_office_ready_for_copy(path: &Path) -> Result<(), String> {
    if !wait_for_office_file_stable(path) {
        return Err(format!(
            "Office file size did not stabilize: {}",
            path.display()
        ));
    }
    if !wait_until_file_readable(path) {
        return Err(format!(
            "Office file still locked or unreadable after wait: {}",
            path.display()
        ));
    }
    if !wait_for_office_file_stable(path) {
        return Err(format!(
            "Office file changed size after unlock: {}",
            path.display()
        ));
    }
    Ok(())
}

/// Resolve `soffice.exe`: env `LIBREOFFICE_SOFFICE`, then config path, then common install dirs.
pub fn resolve_soffice_path(config_override: Option<&str>) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LIBREOFFICE_SOFFICE") {
        let pb = PathBuf::from(p.trim());
        if pb.is_file() {
            return Some(pb);
        }
    }
    if let Some(s) = config_override {
        let t = s.trim();
        if !t.is_empty() {
            let pb = PathBuf::from(t);
            if pb.is_file() {
                return Some(pb);
            }
        }
    }
    #[cfg(windows)]
    {
        for candidate in [
            r"C:\Program Files\LibreOffice\program\soffice.exe",
            r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
        ] {
            let pb = PathBuf::from(candidate);
            if pb.is_file() {
                return Some(pb);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let pb = PathBuf::from("/Applications/LibreOffice.app/Contents/MacOS/soffice");
        if pb.is_file() {
            return Some(pb);
        }
    }
    #[cfg(not(windows))]
    {
        for candidate in ["/usr/bin/soffice", "/usr/local/bin/soffice"] {
            let pb = PathBuf::from(candidate);
            if pb.is_file() {
                return Some(pb);
            }
        }
    }
    None
}

pub fn is_convertible_office_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_lowercase().as_str(),
                "doc" | "docx" | "rtf" | "xls" | "xlsx" | "xlsm" | "ppt" | "pptx" | "odt" | "ods" | "odp"
            )
        })
        .unwrap_or(false)
}

fn is_office_doc(path: &Path) -> bool {
    is_convertible_office_path(path)
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

#[derive(Debug)]
enum ClaimKind {
    Office,
    Pdf,
}

impl ClaimKind {
    fn validate(&self, path: &Path) -> Result<(), String> {
        match self {
            Self::Office if !is_office_doc(path) => Err("Not a convertible Office document".into()),
            Self::Pdf if !is_pdf(path) => Err("Not a PDF".into()),
            _ => Ok(()),
        }
    }

    fn first_try_foreground(&self) -> &'static str {
        match self {
            Self::Office => "office_claim_before_first_try",
            Self::Pdf => "pdf_claim_before_first_try",
        }
    }

    fn retry_foreground(&self) -> &'static str {
        match self {
            Self::Office => "office_claim_retry",
            Self::Pdf => "pdf_claim_retry",
        }
    }

    fn default_ext(&self) -> &'static str {
        match self {
            Self::Office => "docx",
            Self::Pdf => "pdf",
        }
    }
}

/// If no sibling `stem.pdf`, or the Word file is newer than the PDF, convert again.
pub fn needs_conversion(word_path: &Path, pdf_path: &Path) -> bool {
    if !pdf_path.is_file() {
        return true;
    }
    let w = std::fs::metadata(word_path).and_then(|m| m.modified());
    let p = std::fs::metadata(pdf_path).and_then(|m| m.modified());
    match (w, p) {
        (Ok(wt), Ok(pt)) => wt > pt,
        _ => true,
    }
}

pub fn path_is_under_uce_staging(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(".uce_staging")
    })
}

fn path_is_under_filewisely_tree(path: &Path) -> bool {
    let root = Path::new(print_config::FILEWISELY_ROOT);
    match (fs::canonicalize(path), fs::canonicalize(root)) {
        (Ok(p), Ok(r)) => p.starts_with(&r),
        _ => path
            .to_string_lossy()
            .to_lowercase()
            .starts_with(&print_config::FILEWISELY_ROOT.to_lowercase()),
    }
}

/// True if `path` is a file whose parent is exactly `C:\FileWisely\Incoming` (case/normalization aware).
pub fn is_direct_child_of_fw_incoming(path: &Path) -> bool {
    let incoming = PathBuf::from(print_config::FW_OUTPUT_DIR);
    let Some(parent) = path.parent() else {
        return false;
    };
    match (fs::canonicalize(parent), fs::canonicalize(&incoming)) {
        (Ok(p), Ok(i)) => p == i,
        _ => parent
            .to_string_lossy()
            .eq_ignore_ascii_case(&incoming.to_string_lossy()),
    }
}

/// PDF output directory + stem for Office ingestion: Incoming children keep basename; external sources
/// get a unique `fw_*` stem and PDF lands under [`print_config::FW_OUTPUT_DIR`].
pub fn office_output_dir_and_pdf_stem(path: &Path) -> (PathBuf, String) {
    if is_direct_child_of_fw_incoming(path) {
        let out_dir = path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(print_config::FW_OUTPUT_DIR));
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();
        (out_dir, stem)
    } else {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let rnd = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            % 0xFFFFFF) as u32;
        let stem = format!("fw_{ts}_{rnd:06x}");
        (PathBuf::from(print_config::FW_OUTPUT_DIR), stem)
    }
}

/// Staging directory for an Office file about to be claimed from `original`.
fn office_staging_root_for_incoming(original: &Path, out_dir: &Path) -> PathBuf {
    if print_config::ccc_temp_watch_only() {
        out_dir.join(".uce_staging")
    } else if is_direct_child_of_fw_incoming(original) {
        print_config::filewisely_uce_staging_dir()
    } else {
        out_dir.join(".uce_staging")
    }
}

fn rename_or_copy_delete_for_fail(src: &Path, dest: &Path) -> Result<(), String> {
    if let Some(p) = dest.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(e1) => {
            fs::copy(src, dest).map_err(|e| format!("copy failed: {e}; rename was: {e1}"))?;
            fs::remove_file(src).map_err(|e| e.to_string())?;
            Ok(())
        }
    }
}

fn unique_failed_dest(from: &Path, failed: &Path) -> PathBuf {
    let name = from.file_name().unwrap_or_default();
    let dest = failed.join(name);
    if !dest.exists() {
        return dest;
    }
    let stem = from.file_stem().and_then(|s| s.to_str()).unwrap_or("office");
    let ext = from.extension().and_then(|e| e.to_str());
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    match ext {
        Some(e) => failed.join(format!("{stem}_{ts}.{e}")),
        None => failed.join(format!("{stem}_{ts}")),
    }
}

/// Move a failed Office (or staged Office) file into `C:\FileWisely\Failed` with `{stem}.error.json`.
pub fn filewisely_fail_office_path(from: &Path, error_json: &str) -> Result<(), String> {
    if !path_is_under_filewisely_tree(from) {
        return Err(format!(
            "filewisely_fail_office_path: path not under {}: {}",
            print_config::FILEWISELY_ROOT,
            from.display()
        ));
    }
    let failed = print_config::filewisely_failed_dir();
    fs::create_dir_all(&failed).map_err(|e| e.to_string())?;
    let dest = unique_failed_dest(from, &failed);
    rename_or_copy_delete_for_fail(from, &dest)?;
    let stem = dest
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("office");
    let jpath = failed.join(format!("{stem}.error.json"));
    fs::write(&jpath, error_json).map_err(|e| e.to_string())?;
    eprintln!(
        "[UCE] OFFICE_MOVED_TO_FAILED dest={} sidecar={}",
        dest.display(),
        jpath.display()
    );
    Ok(())
}

#[cfg(windows)]
fn hide_staging_dir_once(dir: &Path) {
    use std::os::windows::process::CommandExt;
    let s = dir.to_string_lossy();
    let _ = Command::new("cmd")
        .args(["/C", "attrib", "+h", s.as_ref()])
        .creation_flags(0x0800_0000)
        .output();
}

#[cfg(not(windows))]
fn hide_staging_dir_once(_dir: &Path) {}

#[cfg(windows)]
fn is_sharing_violation(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(32)
}

#[cfg(not(windows))]
fn is_sharing_violation(_e: &std::io::Error) -> bool {
    false
}

/// Rename into staging. On sharing violation, return Err (caller retries). Otherwise try copy+delete (cross-volume).
fn try_claim_rename_or_copy(original: &Path, dest: &Path) -> Result<(), std::io::Error> {
    match fs::rename(original, dest) {
        Ok(()) => Ok(()),
        Err(e) => {
            if is_sharing_violation(&e) {
                return Err(e);
            }
            fs::copy(original, dest)?;
            fs::remove_file(original)?;
            Ok(())
        }
    }
}

/// Move `original` into `staging_root` as soon as the OS allows (first attempt is immediate; no stability wait here).
fn claim_file_to_staging(
    original: &Path,
    staging_root: &Path,
    stem: &str,
    detected_at: Option<Instant>,
    claim_tag: &str,
    kind: ClaimKind,
) -> Result<PathBuf, String> {
    kind.validate(original)?;

    eprintln!(
        "UCE_FILE_COPY_ATTEMPT path={} stem={} claim_tag={} kind={:?}",
        original.display(),
        stem,
        claim_tag,
        kind
    );
    pipeline_stage_diag::record_copy_attempt(&original.to_string_lossy());

    let (interval_ms, max_attempts) = if print_config::ccc_temp_watch_only() {
        (150u64, 34usize)
    } else {
        (35u64, 120usize)
    };

    let ext = original
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or(kind.default_ext());

    let mut last_err: Option<String> = None;
    let mut logged_before_first_try = false;

    for attempt in 0..max_attempts {
        if !original.exists() {
            thread::sleep(Duration::from_millis(interval_ms));
            continue;
        }
        if !original.is_file() {
            thread::sleep(Duration::from_millis(interval_ms));
            continue;
        }

        if !logged_before_first_try {
            foreground_telemetry::log_foreground(kind.first_try_foreground());
            logged_before_first_try = true;
        } else if attempt > 0 && attempt % 25 == 0 {
            foreground_telemetry::log_foreground(kind.retry_foreground());
        }

        fs::create_dir_all(staging_root).map_err(|e| format!("create staging dir: {e}"))?;
        hide_staging_dir_once(staging_root);

        let nano = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .wrapping_add(attempt as u128);
        let dest = staging_root.join(format!("{nano}_{stem}.{ext}"));

        match try_claim_rename_or_copy(original, &dest) {
            Ok(()) => {
                foreground_telemetry::log_claim_telemetry(claim_tag, original, &dest, detected_at);
                if matches!(kind, ClaimKind::Office) {
                    eprintln!(
                        "[UCE] OFFICE_MOVED_TO_STAGING from={} to={}",
                        original.display(),
                        dest.display()
                    );
                }
                eprintln!("UCE_FILE_COPY_SUCCESS staged_path={}", dest.display());
                pipeline_stage_diag::record_copy_success(&dest.to_string_lossy());
                if print_config::ccc_temp_watch_only() {
                    let staging_move_unix_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let last_w = foreground_telemetry::last_winword_foreground_unix_ms();
                    let relation = if last_w == 0 {
                        "unknown"
                    } else if last_w < staging_move_unix_ms {
                        "winword_last_seen_before_staging_move"
                    } else {
                        "winword_last_seen_at_or_after_staging_move"
                    };
                    eprintln!(
                        "[UCE] CCC office lifecycle: moved_to_staging path={}",
                        dest.display()
                    );
                    eprintln!(
                        "[UCE] CCC watch: word_timing staged_path={} last_winword_unix_ms={} staging_move_unix_ms={} relation={}",
                        dest.display(),
                        last_w,
                        staging_move_unix_ms,
                        relation
                    );
                }
                return Ok(dest);
            }
            Err(e) => {
                last_err = Some(format!("{e}"));
                let busy = is_sharing_violation(&e) || e.kind() == ErrorKind::PermissionDenied;
                if busy && (attempt == 0 || attempt % 25 == 24) {
                    let hint = foreground_telemetry::sharing_violation_process_hint();
                    if matches!(kind, ClaimKind::Office) {
                        eprintln!(
                            "[UCE] OFFICE_CLAIM_BUSY attempt={} path={} err={} {}",
                            attempt,
                            original.display(),
                            e,
                            hint
                        );
                    } else {
                        eprintln!(
                            "[UCE] Claim retry {} (file busy): {} — {} | {}",
                            attempt,
                            original.display(),
                            e,
                            hint
                        );
                    }
                }
                thread::sleep(Duration::from_millis(interval_ms));
            }
        }
    }

    if matches!(kind, ClaimKind::Office) {
        let hint = foreground_telemetry::sharing_violation_process_hint();
        eprintln!(
            "[UCE] OFFICE_CLAIM_GAVE_UP path={} {} last_err={}",
            original.display(),
            hint,
            last_err.as_deref().unwrap_or("unknown")
        );
    }

    eprintln!(
        "UCE_FILE_COPY_FAILED path={} last_err={}",
        original.display(),
        last_err.as_deref().unwrap_or("unknown")
    );
    pipeline_stage_diag::record_copy_failure(
        &original.to_string_lossy(),
        last_err.as_deref().unwrap_or("unknown"),
    );

    Err(format!(
        "Could not claim {} after {} attempts (last: {})",
        original.display(),
        max_attempts,
        last_err.unwrap_or_else(|| "unknown".into())
    ))
}

/// Office: move into staging (FileWisely: `C:\FileWisely\.uce_staging`; CCC: `out_dir/.uce_staging`).
pub fn claim_office_from_incoming(
    original: &Path,
    out_dir: &Path,
    pdf_stem: &str,
    detected_at: Option<Instant>,
    claim_tag: &str,
) -> Result<PathBuf, String> {
    let staging_root = office_staging_root_for_incoming(original, out_dir);
    claim_file_to_staging(
        original,
        &staging_root,
        pdf_stem,
        detected_at,
        claim_tag,
        ClaimKind::Office,
    )
}

/// PDF: move into `.uce_staging/` (CCC temp test mode — process only staged file).
pub fn claim_pdf_from_incoming(
    original: &Path,
    out_dir: &Path,
    stem: &str,
    detected_at: Option<Instant>,
    claim_tag: &str,
) -> Result<PathBuf, String> {
    let staging_root = out_dir.join(".uce_staging");
    claim_file_to_staging(
        original,
        &staging_root,
        stem,
        detected_at,
        claim_tag,
        ClaimKind::Pdf,
    )
}

/// Claim only; incoming lock released when this returns. Use with CCC queue worker (no convert here).
pub fn claim_office_for_ccc_queue(
    incoming_path: &Path,
    out_dir: &Path,
    pdf_stem: &str,
    detected_at: Option<Instant>,
    claim_tag: &str,
) -> Result<PathBuf, String> {
    let key = incoming_pipeline_key(incoming_path);
    let Some(_g) = try_acquire_incoming_pipeline(key) else {
        return Err(DUPLICATE_OFFICE_PIPELINE_SKIPPED.to_string());
    };
    claim_office_from_incoming(incoming_path, out_dir, pdf_stem, detected_at, claim_tag)
}

/// Same as [`claim_office_for_ccc_queue`] for PDFs.
pub fn claim_pdf_for_ccc_queue(
    incoming_path: &Path,
    out_dir: &Path,
    stem: &str,
    detected_at: Option<Instant>,
    claim_tag: &str,
) -> Result<PathBuf, String> {
    let key = incoming_pipeline_key(incoming_path);
    let Some(_g) = try_acquire_incoming_pipeline(key) else {
        return Err(DUPLICATE_OFFICE_PIPELINE_SKIPPED.to_string());
    };
    claim_pdf_from_incoming(incoming_path, out_dir, stem, detected_at, claim_tag)
}

/// Full pipeline: claim from Incoming → wait on staged file only → LibreOffice → PDF in `out_dir`.
/// Does **not** convert while the file remains on the visible Incoming path.
/// Paths already under `.uce_staging/` are converted in place (no second claim).
pub fn ingest_office_incoming_to_pdf(
    soffice: &Path,
    incoming_path: &Path,
    out_dir: &Path,
    pdf_stem: &str,
    detected_at: Option<Instant>,
    claim_tag: &str,
) -> Result<PathBuf, String> {
    let key = incoming_pipeline_key(incoming_path);
    let Some(_pipeline) = try_acquire_incoming_pipeline(key) else {
        return Err(DUPLICATE_OFFICE_PIPELINE_SKIPPED.to_string());
    };

    let under_staging = path_is_under_uce_staging(incoming_path);
    let pdf_stem_owned = if under_staging {
        pdf_stem_from_staged_filename(incoming_path)?
    } else {
        pdf_stem.to_string()
    };

    let staged = if under_staging {
        incoming_path.to_path_buf()
    } else {
        match claim_office_from_incoming(
            incoming_path,
            out_dir,
            &pdf_stem_owned,
            detected_at,
            claim_tag,
        ) {
            Ok(p) => p,
            Err(e) => {
                if incoming_path.is_file() && path_is_under_filewisely_tree(incoming_path) {
                    let body = json!({
                        "stage": "office_claim",
                        "claim_tag": claim_tag,
                        "error": e
                    })
                    .to_string();
                    let _ = filewisely_fail_office_path(incoming_path, &body);
                }
                return Err(e);
            }
        }
    };

    let _guard = OFFICE_TO_PDF_LOCK
        .lock()
        .map_err(|_| "Office→PDF lock poisoned".to_string())?;
    let pdf = match convert_staged_office_to_pdf(soffice, &staged, out_dir, pdf_stem_owned.as_str()) {
        Ok(p) => p,
        Err(e) => {
            if staged.is_file() && path_is_under_filewisely_tree(&staged) {
                let body = json!({
                    "stage": "office_convert",
                    "claim_tag": claim_tag,
                    "error": e,
                    "staged": staged.to_string_lossy()
                })
                .to_string();
                let _ = filewisely_fail_office_path(&staged, &body);
            }
            return Err(e);
        }
    };
    foreground_telemetry::log_foreground("office_after_ingest_success");
    Ok(pdf)
}

fn maybe_retain_staged_debug_copy(staged_path: &Path, out_dir: &Path) {
    if !print_config::uce_retain_staging_debug() {
        return;
    }
    let sub = out_dir.join(".uce_debug_retained");
    let _ = fs::create_dir_all(&sub);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let fname = staged_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let dest = sub.join(format!("{ts}_{fname}"));
    if fs::copy(staged_path, &dest).is_ok() {
        eprintln!(
            "[UCE] CCC debug: retained_staging_copy src={} dest={}",
            staged_path.display(),
            dest.display()
        );
    }
}

/// Serialize LibreOffice runs; used by CCC queue worker.
pub fn convert_staged_office_in_queue(
    soffice: &Path,
    staged_path: &Path,
    out_dir: &Path,
    pdf_stem: &str,
) -> Result<PathBuf, String> {
    let _guard = OFFICE_TO_PDF_LOCK
        .lock()
        .map_err(|_| "Office→PDF lock poisoned".to_string())?;
    convert_staged_office_to_pdf(soffice, staged_path, out_dir, pdf_stem)
}

/// Convert a file that already lives under `.uce_staging/`. Waits for stability on `staged_path` only.
pub(crate) fn convert_staged_office_to_pdf(
    soffice: &Path,
    staged_path: &Path,
    out_dir: &Path,
    pdf_stem: &str,
) -> Result<PathBuf, String> {
    if !path_is_under_uce_staging(staged_path) {
        return Err(
            "Internal error: convert_staged_office_to_pdf requires a path under .uce_staging".into(),
        );
    }
    if !staged_path.is_file() {
        return Err("Staged Office file does not exist".into());
    }
    if !is_office_doc(staged_path) {
        return Err("Not a convertible Office file".into());
    }
    fs::create_dir_all(out_dir).map_err(|e| format!("create outdir: {e}"))?;

    let ext = staged_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("docx");
    let out_str = out_dir
        .to_str()
        .ok_or_else(|| "Invalid output dir".to_string())?;

    let log_from = staged_path.display().to_string();

    wait_office_ready_for_copy(staged_path).map_err(|e| {
        eprintln!("[UCE] OFFICE_CONVERT_FAILED staged={} err=staged_not_ready {e}", staged_path.display());
        e
    })?;

    if print_config::ccc_temp_watch_only() {
        eprintln!(
            "[UCE] CCC office lifecycle: stability_passed path={}",
            staged_path.display()
        );
    }

    eprintln!(
        "[UCE] OFFICE_CONVERT_STARTED staged={} out_dir={} pdf_stem={}",
        staged_path.display(),
        out_dir.display(),
        pdf_stem
    );

    if print_config::ccc_temp_watch_only() {
        eprintln!(
            "[UCE] CCC office lifecycle: conversion_started path={} pdf_stem={}",
            staged_path.display(),
            pdf_stem
        );
    }

    let work_stem = format!("{pdf_stem}_ucework");
    let work_path = out_dir.join(format!("{work_stem}.{ext}"));
    let _ = fs::remove_file(&work_path);
    fs::copy(staged_path, &work_path).map_err(|e| format!("Copy for LibreOffice: {e}"))?;

    let work_input_str = work_path
        .to_str()
        .ok_or_else(|| "Invalid temp path".to_string())?;

    let pdf_from_lo = out_dir.join(format!("{work_stem}.pdf"));
    let pdf_final = out_dir.join(format!("{pdf_stem}.pdf"));

    const RETRIES: usize = 4;
    let mut last_err = String::new();

    for attempt in 0..RETRIES {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(900));
            let _ = fs::remove_file(&pdf_from_lo);
            let _ = fs::remove_file(&work_path);
            wait_office_ready_for_copy(staged_path).map_err(|e| {
                eprintln!("[UCE] Staged Office not ready for copy (retry): {e}");
                e
            })?;
            fs::copy(staged_path, &work_path).map_err(|e| format!("Copy for LibreOffice: {e}"))?;
        }

        let mut cmd = Command::new(soffice);
        cmd.args([
            "--headless",
            "--invisible",
            "--nologo",
            "--nofirststartwizard",
            "--convert-to",
            "pdf",
            work_input_str,
            "--outdir",
            out_str,
        ])
        .env("SAL_USE_VCLPLUGIN", "svp");

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }

        let output = cmd.output().map_err(|e| {
            format!("Failed to run LibreOffice: {e}. Is it installed?")
        })?;

        if output.status.success() && pdf_from_lo.is_file() {
            let _ = fs::remove_file(&pdf_final);
            fs::rename(&pdf_from_lo, &pdf_final).map_err(|e| format!("rename PDF: {e}"))?;
            let _ = fs::remove_file(&work_path);
            if print_config::ccc_temp_watch_only() {
                eprintln!(
                    "[UCE] CCC office lifecycle: conversion_finished path={} pdf={}",
                    staged_path.display(),
                    pdf_final.display()
                );
            }
            maybe_retain_staged_debug_copy(staged_path, out_dir);
            let _ = fs::remove_file(staged_path);
            eprintln!(
                "[UCE] OFFICE_CONVERT_FINISHED staged={} ok=true",
                log_from
            );
            eprintln!(
                "[UCE] OFFICE_CONVERT_OUTPUT_PATH pdf={}",
                pdf_final.display()
            );
            return Ok(pdf_final);
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        last_err = format!(
            "LibreOffice exit {}: stderr={} stdout={}",
            output.status,
            stderr.trim(),
            stdout.trim()
        );
    }

    let _ = fs::remove_file(&work_path);
    let _ = fs::remove_file(&pdf_from_lo);
    if pdf_final.is_file() {
        if print_config::ccc_temp_watch_only() {
            eprintln!(
                "[UCE] CCC office lifecycle: conversion_finished path={} pdf={}",
                staged_path.display(),
                pdf_final.display()
            );
        }
        maybe_retain_staged_debug_copy(staged_path, out_dir);
        let _ = fs::remove_file(staged_path);
        eprintln!(
            "[UCE] OFFICE_CONVERT_FINISHED staged={} ok=true (partial_retry)",
            log_from
        );
        eprintln!(
            "[UCE] OFFICE_CONVERT_OUTPUT_PATH pdf={}",
            pdf_final.display()
        );
        return Ok(pdf_final);
    }

    eprintln!(
        "[UCE] OFFICE_CONVERT_FAILED staged={} err={}",
        log_from,
        last_err.trim()
    );
    Err(last_err)
}
