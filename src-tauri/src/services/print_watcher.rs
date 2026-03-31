//! Watch configured roots for new PDFs / Office files and hook the UCE pipeline.
//!
//! Does **not** install a printer driver — shops use “Microsoft Print to PDF” (or similar) renamed to
//! “FileWisely Printer” and point saves at this folder when possible.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult};
use serde_json::json;
use tauri::Emitter;

use crate::config::print_config::{self, FW_PRINTER_DISPLAY_NAME};
use crate::pdf_watch_config;
use crate::services::ccc_batch;
use crate::services::converter;
use crate::services::foreground_telemetry;
use crate::services::incoming_emit;
use crate::services::incoming_unique_rename;
use crate::services::office_printer_route;

fn extension_kind(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        "pdf" => Some("pdf"),
        "doc" | "docx" | "rtf" => Some("office"),
        _ => None,
    }
}

fn office_ext_lower(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default()
}

fn office_debug_log_all_sources(app: &tauri::AppHandle) -> bool {
    print_config::uce_debug_office_sources()
        || pdf_watch_config::load_pdf_watch_config(app).office_debug_log_all_detected
}

fn log_office_source_detected(path: &Path, rule: &str) {
    let ext = office_ext_lower(path);
    eprintln!(
        "[UCE] OFFICE_SOURCE_DETECTED OFFICE_SOURCE_PATH={} OFFICE_SOURCE_EXT={} OFFICE_SOURCE_MATCHED_RULE={}",
        path.display(),
        ext,
        rule
    );
}

fn log_office_debug_source_seen(path: &Path, rule: &str) {
    let ext = office_ext_lower(path);
    eprintln!(
        "[UCE] OFFICE_DEBUG_SOURCE_SEEN OFFICE_SOURCE_PATH={} OFFICE_SOURCE_EXT={} OFFICE_SOURCE_MATCHED_RULE={}",
        path.display(),
        ext,
        rule
    );
}

fn emit_office_claim_failed_prompt(app: &tauri::AppHandle, path: &Path, cfg: &pdf_watch_config::PdfWatchConfig) {
    if !cfg.office_print_prompt_fallback {
        return;
    }
    let path_str = path.to_string_lossy().to_string();
    let _ = app.emit(
        "uce-office-print-prompt",
        json!({
            "path": path_str,
            "message": "Send this document to FileWisely?",
            "reason": "claim_failed",
        }),
    );
}

/// Wait until the file exists, is non-empty, and **byte size unchanged** for several consecutive
/// reads (CCC burst writes can trigger create events before the spooler finishes).
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

/// Word → PDF on a background thread so the debouncer is not blocked by LibreOffice (PDFs still process).
fn process_filewisely_office_incoming(app: tauri::AppHandle, path: PathBuf, matched_rule: &'static str) {
    let t0 = Instant::now();
    let original_display = path.display().to_string();
    log_office_source_detected(&path, matched_rule);
    eprintln!("[UCE] OFFICE_DETECTED path={}", original_display);
    foreground_telemetry::log_foreground("office_file_detected");
    let cfg = pdf_watch_config::load_pdf_watch_config(&app);

    match office_printer_route::route_office_document_printer_first(
        &app,
        &path,
        &cfg,
        matched_rule,
    ) {
        office_printer_route::OfficePrinterFirstResult::SkipDuplicateWindow => return,
        office_printer_route::OfficePrinterFirstResult::HandledByPrinter => return,
        office_printer_route::OfficePrinterFirstResult::FallThroughToStaging => {}
    }

    if !cfg.word_to_pdf_enabled {
        eprintln!(
            "[UCE] OFFICE_PIPELINE_RESULT path={} result=skipped reason=word_to_pdf_disabled",
            original_display
        );
        return;
    }
    let Some(soffice) = converter::resolve_soffice_path(cfg.libreoffice_path.as_deref()) else {
        eprintln!(
            "[UCE] OFFICE_PIPELINE_RESULT path={} result=skipped reason=libreoffice_not_found",
            original_display
        );
        eprintln!("UCE print watcher: LibreOffice not found; Word → PDF deferred to poll");
        return;
    };
    let (out_dir, stem) = converter::office_output_dir_and_pdf_stem(&path);
    let pdf_path = out_dir.join(format!("{stem}.pdf"));
    if !converter::needs_conversion(&path, &pdf_path) {
        let path_str = pdf_path.to_string_lossy().to_string();
        eprintln!(
            "[UCE] OFFICE_ENQUEUED_AS_PDF path={} reason=existing_or_current_pdf",
            path_str
        );
        incoming_emit::emit_uce_incoming_pdf(&app, path_str);
        return;
    }
    match converter::ingest_office_incoming_to_pdf(
        &soffice,
        &path,
        &out_dir,
        stem.as_str(),
        Some(t0),
        "fs_watcher",
    ) {
        Ok(pdf_out) => {
            eprintln!(
                "[UCE] OFFICE_ENQUEUED_AS_PDF path={}",
                pdf_out.display()
            );
            eprintln!(
                "[UCE] OFFICE_INGESTION_MODE=staging_convert success pdf={}",
                pdf_out.display()
            );
            if cfg.delete_word_after_convert {
                let _ = fs::remove_file(&path);
            }
            let path_str = pdf_out.to_string_lossy().to_string();
            incoming_emit::emit_uce_incoming_pdf(&app, path_str);
        }
        Err(e) if e == converter::DUPLICATE_OFFICE_PIPELINE_SKIPPED => {
            eprintln!(
                "[UCE] OFFICE_PIPELINE_RESULT path={} result=skipped reason=duplicate_pipeline",
                original_display
            );
        }
        Err(e) => {
            eprintln!(
                "[UCE] OFFICE_FINAL_ERROR path={} err={}",
                original_display, e
            );
            eprintln!(
                "[UCE] OFFICE_PIPELINE_RESULT path={} result=failed",
                original_display
            );
            if e.contains("Could not claim") && path.is_file() {
                emit_office_claim_failed_prompt(&app, &path, &cfg);
            }
        }
    }
}

fn handle_path(
    app: &tauri::AppHandle,
    path: std::path::PathBuf,
    roots: &[(PathBuf, &'static str)],
) {
    if print_config::ccc_temp_watch_only() {
        if path.is_file() && !converter::path_is_under_uce_staging(&path) {
            foreground_telemetry::spawn_foreground_debug_poll_after_detection();
            ccc_batch::handle_ccc_temp_file(app, path);
        }
        return;
    }

    let Some(kind) = extension_kind(&path) else {
        return;
    };

    let matched_rule = pdf_watch_config::resolve_office_source_rule(&path, roots);

    if kind == "office" && office_debug_log_all_sources(app) {
        log_office_debug_source_seen(&path, matched_rule);
    }

    foreground_telemetry::spawn_foreground_debug_poll_after_detection();

    match kind {
        "pdf" => {
            eprintln!("[UCE] File detected: {}", path.display());
            let path = incoming_unique_rename::unique_rename_incoming_pdf_if_needed(path);
            if !path.is_file() {
                return;
            }
            if !wait_for_pdf_file_stable(&path) {
                eprintln!(
                    "[UCE] File stable: FAILED (still writing or locked): {}",
                    path.display()
                );
                return;
            }
            eprintln!("[UCE] File stable: {}", path.display());
            let path_str = path.to_string_lossy().to_string();
            incoming_emit::emit_uce_incoming_pdf(app, path_str);
        }
        "office" => {
            if !path.is_file() {
                return;
            }
            let path = incoming_unique_rename::unique_rename_incoming_office_if_needed(path);
            if !path.is_file() {
                return;
            }
            let app = app.clone();
            let rule = matched_rule;
            thread::spawn(move || {
                process_filewisely_office_incoming(app, path, rule);
            });
        }
        _ => {}
    }
}

/// Spawn a background thread that debounce-watches Office/PDF roots.
#[cfg(windows)]
pub fn spawn_print_watcher(app: tauri::AppHandle) {
    if print_config::ccc_temp_watch_only() {
        ccc_batch::init_ccc_batch_subsystems(&app);
    }
    thread::spawn(move || {
        let app = app;
        let roots_vec = pdf_watch_config::office_intercept_watch_roots(&app);
        let roots_arc = Arc::new(roots_vec);
        let roots_for_summary = Arc::clone(&roots_arc);

        eprintln!(
            "[UCE] OFFICE_WATCH_SUMMARY roots={}",
            roots_for_summary.len()
        );
        for (p, rule) in roots_for_summary.iter() {
            eprintln!(
                "[UCE] OFFICE_WATCH_ROOT path={} OFFICE_SOURCE_MATCHED_RULE={}",
                p.display(),
                rule
            );
        }

        // Short debounce so Office files are claimed into `.uce_staging` soon after the OS creates them.
        // PDFs still wait for byte stability inside `handle_path`.
        let debounce_ms: u64 = if print_config::ccc_temp_watch_only() {
            50
        } else {
            300
        };

        let app_debounce = app.clone();
        let roots_debounce = Arc::clone(&roots_arc);
        let mut debouncer = match new_debouncer(Duration::from_millis(debounce_ms), move |res: DebounceEventResult| {
            let roots = roots_debounce.as_slice();
            match res {
                Ok(events) => {
                    for ev in events {
                        handle_path(&app_debounce, ev.path, roots);
                    }
                }
                Err(e) => eprintln!("UCE print watcher: {e}"),
            }
        }) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("UCE print watcher: debouncer failed: {e}");
                return;
            }
        };

        for (root, _rule) in roots_arc.iter() {
            if let Err(e) = fs::create_dir_all(root) {
                eprintln!(
                    "UCE print watcher: could not ensure watch dir {}: {}",
                    root.display(),
                    e
                );
            }
            if let Err(e) = debouncer
                .watcher()
                .watch(root.as_path(), RecursiveMode::Recursive)
            {
                eprintln!(
                    "UCE print watcher: watch failed for {}: {e}",
                    root.display()
                );
            }
        }

        eprintln!(
            "UCE print watcher: watching {} root(s) — CCC: use a virtual PDF printer renamed to \"{}\"{}",
            roots_arc.len(),
            FW_PRINTER_DISPLAY_NAME,
            if print_config::ccc_temp_watch_only() {
                " [UCE_CCC_TEMP_WATCH_ONLY] — Word/PDF saves must land in CCC temp; C:\\FileWisely\\Incoming is not watched in this mode"
            } else {
                ""
            }
        );

        loop {
            thread::sleep(Duration::from_secs(3600));
        }
    });
}

#[cfg(not(windows))]
pub fn spawn_print_watcher(_app: tauri::AppHandle) {}

/// Alias for [`spawn_print_watcher`] — matches FileWisely “Desktop OS” deployment docs.
#[cfg(windows)]
pub fn start_print_watcher(app: tauri::AppHandle) {
    spawn_print_watcher(app);
}

#[cfg(not(windows))]
pub fn start_print_watcher(_app: tauri::AppHandle) {}
