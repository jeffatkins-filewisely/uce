//! Defaults for FileWisely print capture (CCC → virtual PDF printer → this folder → UCE).
//!
//! **Shop setup:** Add **Microsoft Print to PDF** (or PDF24 / Bullzip with auto-save), rename the printer
//! to [`FW_PRINTER_DISPLAY_NAME`], and set its default save folder to [`FW_OUTPUT_DIR`] when the driver allows it.
//! Phase 2 can swap to a driver with silent auto-save to remove the “Save as” dialog.

use std::path::PathBuf;

/// Shop root for FileWisely folders (Incoming, staging, Processed, Failed).
pub const FILEWISELY_ROOT: &str = r"C:\FileWisely";

/// Ingestion folder watched by UCE (Rust watcher + JS poll). Create at install time.
pub const FW_OUTPUT_DIR: &str = r"C:\FileWisely\Incoming";

/// Machine-local seed written by `install.ps1` / `seed-uce-pdf-watch.ps1` (merged at runtime with per-user `uce-pdf-watch.json`).
pub fn filewisely_pdf_watch_seed_path() -> PathBuf {
    PathBuf::from(FILEWISELY_ROOT)
        .join("App")
        .join("uce-pdf-watch.seed.json")
}

/// Office files from Incoming are claimed here **before** stability/convert (never convert from Incoming).
pub fn filewisely_uce_staging_dir() -> PathBuf {
    PathBuf::from(r"C:\FileWisely\.uce_staging")
}

/// Successful `fw_*.pdf` uploads are moved here (sibling of [`FW_OUTPUT_DIR`]).
pub fn filewisely_processed_dir() -> PathBuf {
    PathBuf::from(r"C:\FileWisely\Processed")
}

/// Failed uploads get the PDF + `{stem}.error.json` (sibling of [`FW_OUTPUT_DIR`]).
pub fn filewisely_failed_dir() -> PathBuf {
    PathBuf::from(r"C:\FileWisely\Failed")
}

/// Display name shops should assign to their virtual PDF printer (rename in Windows “Devices and Printers”).
pub const FW_PRINTER_DISPLAY_NAME: &str = "FileWisely Printer";

/// When set (`UCE_CCC_TEMP_WATCH_ONLY=1`), UCE watches only `%LOCALAPPDATA%\Temp\CCC`, narrows PDF polling to that tree,
/// and stages PDFs from the watcher before processing (see `print_watcher` + `claim_pdf_from_incoming`).
pub fn ccc_temp_watch_only() -> bool {
    std::env::var("UCE_CCC_TEMP_WATCH_ONLY")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Resolved CCC temp path (typically `C:\Users\<user>\AppData\Local\Temp\CCC`).
pub fn ccc_temp_watch_path() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(|b| PathBuf::from(b).join("Temp").join("CCC"))
        .unwrap_or_else(|_| PathBuf::from(r"C:\Users\jeff\AppData\Local\Temp\CCC"))
}

/// Root folder the fs watcher and `try_convert_incoming_word_files_now` use.
pub fn watched_incoming_root() -> PathBuf {
    if ccc_temp_watch_only() {
        ccc_temp_watch_path()
    } else {
        PathBuf::from(FW_OUTPUT_DIR)
    }
}

/// When set (`UCE_DEBUG_BURST=1`), log directory snapshots every 250ms for 10s after first file in a burst window.
pub fn uce_debug_burst() -> bool {
    std::env::var("UCE_DEBUG_BURST")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// When set with CCC temp-only mode, keep copies of staged originals under `.uce_debug_retained/` until conversion completes.
pub fn uce_retain_staging_debug() -> bool {
    ccc_temp_watch_only()
        && std::env::var("UCE_RETAIN_STAGING_DEBUG")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
}

/// Log every `.doc` / `.docx` / `.rtf` seen under Office watch roots (even when skipped).
pub fn uce_debug_office_sources() -> bool {
    std::env::var("UCE_DEBUG_OFFICE_SOURCES")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// When set to a positive integer, UCE periodically deletes **files** in
/// [`filewisely_processed_dir`] older than this many days (mtime). Example: `UCE_PROCESSED_RETENTION_DAYS=90`.
/// Unset or `0` = no automatic cleanup (Processed can grow without bound).
pub fn uce_processed_retention_days() -> Option<u64> {
    std::env::var("UCE_PROCESSED_RETENTION_DAYS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&d| d > 0)
}

/// Same as [`uce_processed_retention_days`] but for [`filewisely_failed_dir`] (failed PDFs + `.error.json`).
/// If unset, Failed uses the **Processed** retention when that is set, so one variable can cover both.
pub fn uce_failed_retention_days() -> Option<u64> {
    std::env::var("UCE_FAILED_RETENTION_DAYS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&d| d > 0)
}
