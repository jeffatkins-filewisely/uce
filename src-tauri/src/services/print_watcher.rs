//! Watch configured roots for new PDFs / Office files and hook the UCE pipeline.
//!
//! Does **not** install a printer driver — shops use “Microsoft Print to PDF” (or similar) renamed to
//! “FileWisely Printer” and point saves at this folder when possible.

use std::fs;
use std::path::{Path, PathBuf};
use std::collections::HashSet;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult};
use serde_json::json;
use tauri::Emitter;

use crate::config::print_config::{self, FW_PRINTER_DISPLAY_NAME};
use crate::pdf_watch_config;
use crate::services::capture_pipeline_status;
use crate::services::ccc_batch;
use crate::services::pipeline_stage_diag;
use crate::services::converter;
use crate::services::foreground_telemetry;
use crate::services::incoming_emit;
use crate::services::incoming_unique_rename;
use crate::services::ccc_capture_diag;
use crate::services::office_printer_route;

fn extension_kind(path: &Path) -> Option<&'static str> {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
    {
        return Some("pdf");
    }
    if converter::is_convertible_office_path(path) {
        return Some("office");
    }
    if converter::is_convertible_image_path(path) {
        return Some("image");
    }
    None
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
        log_general_pdf_captured_if_needed(matched_rule, &path_str);
        incoming_emit::emit_uce_incoming_pdf_detailed(
            &app,
            path_str,
            "office_convert",
            Some(matched_rule),
        );
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
            log_general_pdf_captured_if_needed(matched_rule, &path_str);
            incoming_emit::emit_uce_incoming_pdf_detailed(
                &app,
                path_str,
                "office_convert",
                Some(matched_rule),
            );
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

fn log_general_pdf_captured_if_needed(matched_rule: &str, pdf_path: &str) {
    if pdf_watch_config::is_general_capture_rule(matched_rule) {
        eprintln!("UCE_GENERAL_FILE_CAPTURED path={}", pdf_path);
    }
}

fn process_scan_image_incoming(app: tauri::AppHandle, path: PathBuf, matched_rule: &'static str) {
    let original_display = path.display().to_string();
    eprintln!(
        "[UCE] SCAN_IMAGE_DETECTED path={} matched_rule={}",
        original_display, matched_rule
    );
    if !wait_for_pdf_file_stable(&path) {
        eprintln!(
            "UCE_FILE_REJECTED path={} reason=scan_image_not_stable",
            original_display
        );
        return;
    }
    let cfg = pdf_watch_config::load_pdf_watch_config(&app);
    let Some(soffice) = converter::resolve_soffice_path(cfg.libreoffice_path.as_deref()) else {
        eprintln!(
            "[UCE] SCAN_IMAGE_PIPELINE_RESULT path={} result=skipped reason=libreoffice_not_found",
            original_display
        );
        return;
    };
    let (out_dir, stem) = converter::office_output_dir_and_pdf_stem(&path);
    match converter::ingest_image_incoming_to_pdf(&soffice, &path, &out_dir, stem.as_str()) {
        Ok(pdf_out) => {
            let path_str = pdf_out.to_string_lossy().to_string();
            eprintln!(
                "[UCE] SCAN_IMAGE_ENQUEUED_AS_PDF src={} pdf={}",
                original_display, path_str
            );
            incoming_emit::emit_uce_incoming_pdf_detailed(
                &app,
                path_str,
                "scan_image_convert",
                Some(matched_rule),
            );
        }
        Err(e) if e == converter::DUPLICATE_OFFICE_PIPELINE_SKIPPED => {
            eprintln!(
                "[UCE] SCAN_IMAGE_PIPELINE_RESULT path={} result=skipped reason=duplicate_pipeline",
                original_display
            );
        }
        Err(e) => {
            eprintln!(
                "[UCE] SCAN_IMAGE_PIPELINE_RESULT path={} result=failed err={e}",
                original_display
            );
        }
    }
}

fn handle_path(app: &tauri::AppHandle, path: std::path::PathBuf) {
    let path_disp = path.to_string_lossy().into_owned();
    eprintln!(
        "UCE_FILE_DETECTED_RAW path={} is_file={}",
        path.display(),
        path.is_file()
    );
    pipeline_stage_diag::record_detected(&path_disp);

    if print_config::ccc_temp_watch_only() {
        if !path.is_file() {
            eprintln!(
                "UCE_FILE_REJECTED path={} reason=not_a_file_ccc_temp_mode",
                path.display()
            );
            pipeline_stage_diag::record_rejected(&path_disp, "not_a_file_ccc_temp_mode");
            return;
        }
        if converter::path_is_under_uce_staging(&path) {
            eprintln!(
                "UCE_FILE_REJECTED path={} reason=uce_staging_internal",
                path.display()
            );
            pipeline_stage_diag::record_rejected(&path_disp, "uce_staging_internal");
            return;
        }
        foreground_telemetry::spawn_foreground_debug_poll_after_detection();
        ccc_batch::handle_ccc_temp_file(app, path);
        return;
    }

    let roots = pdf_watch_config::office_intercept_watch_roots(app);

    let cfg = pdf_watch_config::load_pdf_watch_config(app);

    let Some(kind) = extension_kind(&path) else {
        eprintln!(
            "UCE_FILE_REJECTED path={} reason=no_pdf_office_image_extension",
            path.display()
        );
        pipeline_stage_diag::record_rejected(&path_disp, "no_pdf_office_image_extension");
        return;
    };

    let matched_rule = pdf_watch_config::resolve_office_source_rule(&path, roots.as_slice());
    let is_general = pdf_watch_config::is_general_capture_rule(matched_rule);

    eprintln!(
        "UCE_PIPELINE_CONTEXT path={} kind={} matched_rule={}",
        path.display(),
        kind,
        matched_rule
    );

    if is_general {
        eprintln!("UCE_GENERAL_FILE_SEEN path={}", path.display());
        if pdf_watch_config::should_ignore_general_document_path(&path) {
            eprintln!(
                "UCE_GENERAL_FILE_IGNORED path={} reason=ignore_pattern",
                path.display()
            );
            eprintln!(
                "UCE_FILE_REJECTED path={} reason=ignore_pattern_general",
                path.display()
            );
            return;
        }
    }

    if kind == "office" && office_debug_log_all_sources(app) {
        log_office_debug_source_seen(&path, matched_rule);
    }

    foreground_telemetry::spawn_foreground_debug_poll_after_detection();

    match kind {
        "pdf" => {
            eprintln!("[UCE] File detected: {}", path.display());
            ccc_capture_diag::record_ccc_file_seen(&path, Some(matched_rule));

            if is_general {
                if let Ok(m) = fs::metadata(&path) {
                    let min_b = pdf_watch_config::effective_general_min_bytes(&cfg)
                        .max(pdf_watch_config::min_pdf_bytes(&cfg));
                    if m.len() < min_b {
                        eprintln!(
                            "UCE_GENERAL_FILE_IGNORED path={} reason=too_small bytes={}",
                            path.display(),
                            m.len()
                        );
                        eprintln!(
                            "UCE_FILE_REJECTED path={} reason=too_small bytes={} min_b={}",
                            path.display(),
                            m.len(),
                            min_b
                        );
                        pipeline_stage_diag::record_rejected(
                            &path_disp,
                            &format!("too_small bytes={} min_b={}", m.len(), min_b),
                        );
                        return;
                    }
                }
            }

            let path = incoming_unique_rename::unique_rename_incoming_pdf_if_needed(path);
            let path_disp_pdf = path.to_string_lossy().into_owned();
            if !path.is_file() {
                eprintln!(
                    "UCE_FILE_REJECTED path={} reason=vanished_after_incoming_rename",
                    path.display()
                );
                pipeline_stage_diag::record_rejected(&path_disp_pdf, "vanished_after_incoming_rename");
                return;
            }
            if !wait_for_pdf_file_stable(&path) {
                if is_general {
                    eprintln!(
                        "UCE_GENERAL_FILE_IGNORED path={} reason=not_stable",
                        path.display()
                    );
                }
                eprintln!(
                    "[UCE] File stable: FAILED (still writing or locked): {}",
                    path.display()
                );
                eprintln!(
                    "UCE_FILE_REJECTED path={} reason=not_stable_or_locked",
                    path.display()
                );
                pipeline_stage_diag::record_rejected(&path_disp_pdf, "not_stable_or_locked");
                return;
            }
            eprintln!("[UCE] File stable: {}", path.display());
            let path_str = path.to_string_lossy().to_string();
            eprintln!(
                "UCE_FILE_ACCEPTED path={} kind=pdf matched_rule={}",
                path.display(),
                matched_rule
            );
            pipeline_stage_diag::record_accepted(
                &path_str,
                Some(format!("kind=pdf matched_rule={matched_rule}")),
            );
            log_general_pdf_captured_if_needed(matched_rule, &path_str);
            incoming_emit::emit_uce_incoming_pdf_detailed(
                app,
                path_str,
                "watcher_pdf",
                Some(matched_rule),
            );
        }
        "office" => {
            if is_general {
                eprintln!(
                    "UCE_FILE_REJECTED path={} reason=office_skipped_general_folder matched_rule={}",
                    path.display(),
                    matched_rule
                );
                pipeline_stage_diag::record_rejected(&path_disp, "office_skipped_general_folder");
                return;
            }
            let office_disp = path.to_string_lossy().into_owned();
            if !path.is_file() {
                eprintln!(
                    "UCE_FILE_REJECTED path={} reason=not_a_file_office_branch",
                    path.display()
                );
                pipeline_stage_diag::record_rejected(&office_disp, "not_a_file_office_branch");
                return;
            }

            if is_general {
                if let Ok(m) = fs::metadata(&path) {
                    let min_b = pdf_watch_config::effective_general_min_bytes(&cfg);
                    if m.len() < min_b {
                        eprintln!(
                            "UCE_GENERAL_FILE_IGNORED path={} reason=too_small bytes={}",
                            path.display(),
                            m.len()
                        );
                        eprintln!(
                            "UCE_FILE_REJECTED path={} reason=too_small_office bytes={} min_b={}",
                            path.display(),
                            m.len(),
                            min_b
                        );
                        pipeline_stage_diag::record_rejected(
                            &office_disp,
                            &format!("too_small_office bytes={} min_b={}", m.len(), min_b),
                        );
                        return;
                    }
                }
            }

            let path = incoming_unique_rename::unique_rename_incoming_office_if_needed(path);
            let office_final = path.to_string_lossy().into_owned();
            if !path.is_file() {
                eprintln!(
                    "UCE_FILE_REJECTED path={} reason=vanished_after_office_rename",
                    path.display()
                );
                pipeline_stage_diag::record_rejected(&office_final, "vanished_after_office_rename");
                return;
            }
            pipeline_stage_diag::record_accepted(
                &office_final,
                Some(format!("kind=office matched_rule={matched_rule}")),
            );
            ccc_capture_diag::record_ccc_file_seen(&path, Some(matched_rule));
            let app = app.clone();
            let rule = matched_rule;
            thread::spawn(move || {
                process_filewisely_office_incoming(app, path, rule);
            });
        }
        "image" => {
            if !pdf_watch_config::is_scan_source_rule(matched_rule)
                && matched_rule != "ccc_temp"
                && matched_rule != "ccc_autodiscovered"
            {
                eprintln!(
                    "UCE_FILE_REJECTED path={} reason=image_not_from_scan_root matched_rule={}",
                    path.display(),
                    matched_rule
                );
                pipeline_stage_diag::record_rejected(&path_disp, "image_not_from_scan_root");
                return;
            }
            if !path.is_file() {
                pipeline_stage_diag::record_rejected(&path_disp, "not_a_file_image_branch");
                return;
            }
            pipeline_stage_diag::record_accepted(
                &path_disp,
                Some(format!("kind=image matched_rule={matched_rule}")),
            );
            let app = app.clone();
            let rule = matched_rule;
            thread::spawn(move || {
                process_scan_image_incoming(app, path, rule);
            });
        }
        _ => {}
    }
}

/// Public entry so source harvest can ingest a just-discovered file immediately.
pub fn ingest_path_now(app: &tauri::AppHandle, path: PathBuf) {
    handle_path(app, path);
}

/// Spawn a background thread that debounce-watches Office/PDF roots.
#[cfg(windows)]
pub fn spawn_print_watcher(app: tauri::AppHandle) -> Result<(), String> {
    if print_config::ccc_temp_watch_only() {
        ccc_batch::init_ccc_batch_subsystems(&app);
    }
    thread::Builder::new()
        .name("uce-print-watcher".into())
        .spawn(move || {
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
        let mut debouncer = match new_debouncer(Duration::from_millis(debounce_ms), move |res: DebounceEventResult| {
            match res {
                Ok(events) => {
                    for ev in events {
                        handle_path(&app_debounce, ev.path);
                    }
                }
                Err(e) => eprintln!("UCE print watcher: {e}"),
            }
        }) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("UCE_CAPTURE_PIPELINE_FAILED_TO_START phase=debouncer error={e}");
                eprintln!("UCE print watcher: debouncer failed: {e}");
                capture_pipeline_status::set_failed(format!("debouncer: {e}"));
                return;
            }
        };

        eprintln!("UCE_PRINT_WATCHER_STARTED");
        capture_pipeline_status::set_running();

        let ccc_temp = print_config::ccc_temp_watch_path();
        for (root, rule) in roots_arc.iter() {
            let is_ccc_appdata_temp = root.to_string_lossy().to_lowercase()
                == ccc_temp.to_string_lossy().to_lowercase()
                || pdf_watch_config::paths_canon_equal(root, &ccc_temp);
            let existed_before = root.exists();
            if let Err(e) = fs::create_dir_all(root) {
                eprintln!(
                    "UCE watch prep failed: could not ensure watch dir {}: {}",
                    root.display(),
                    e
                );
                continue;
            }
            if is_ccc_appdata_temp {
                if existed_before {
                    eprintln!(
                        "UCE_CCC_TEMP_FOLDER_DISCOVERED path={}",
                        root.display()
                    );
                } else {
                    eprintln!(
                        "UCE_CCC_TEMP_FOLDER_MISSING_CREATED path={}",
                        root.display()
                    );
                }
            }
            match debouncer
                .watcher()
                .watch(root.as_path(), RecursiveMode::Recursive)
            {
                Ok(()) => {
                    eprintln!(
                        "UCE_WATCH_ATTACHED path={} OFFICE_SOURCE_MATCHED_RULE={}",
                        root.display(),
                        rule
                    );
                    if is_ccc_appdata_temp {
                        eprintln!(
                            "UCE_CCC_TEMP_WATCH_ROOT_CONFIGURED path={}",
                            root.display()
                        );
                    }
                }
                Err(e) => {
                    eprintln!(
                        "UCE_WATCH_ATTACH_FAILED path={} err={}",
                        root.display(),
                        e
                    );
                    eprintln!(
                        "UCE print watcher: watch failed for {}: {e}",
                        root.display()
                    );
                }
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

        fn canon_watch_key(p: &Path) -> String {
            fs::canonicalize(p)
                .unwrap_or_else(|_| p.to_path_buf())
                .to_string_lossy()
                .to_lowercase()
        }

        let mut watched_keys: HashSet<String> = roots_arc
            .iter()
            .map(|(p, _)| canon_watch_key(p))
            .collect();

        let mut ticks: u32 = 0;
        loop {
            thread::sleep(Duration::from_secs(15));
            ticks = ticks.wrapping_add(1);
            if ticks % 4 != 0 {
                continue;
            }
            let fresh = pdf_watch_config::office_intercept_watch_roots(&app);
            for (root, rule) in fresh {
                let key = canon_watch_key(&root);
                if watched_keys.contains(&key) {
                    continue;
                }
                let _ = fs::create_dir_all(&root);
                match debouncer
                    .watcher()
                    .watch(root.as_path(), RecursiveMode::Recursive)
                {
                    Ok(()) => {
                        watched_keys.insert(key);
                        eprintln!(
                            "UCE_WATCH_ATTACHED path={} OFFICE_SOURCE_MATCHED_RULE={}",
                            root.display(),
                            rule
                        );
                        eprintln!(
                            "UCE_WATCH_ROOT_REFRESH_ADDED path={} OFFICE_SOURCE_MATCHED_RULE={}",
                            root.display(),
                            rule
                        );
                    }
                    Err(e) => {
                        eprintln!(
                            "UCE_WATCH_ATTACH_FAILED path={} err={}",
                            root.display(),
                            e
                        );
                        eprintln!(
                            "UCE_WATCH_ROOT_REFRESH watch failed {}: {e}",
                            root.display()
                        );
                    }
                }
            }
        }
    })
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(windows))]
pub fn spawn_print_watcher(_app: tauri::AppHandle) -> Result<(), String> {
    Ok(())
}

/// Alias for [`spawn_print_watcher`] — matches FileWisely “Desktop OS” deployment docs.
#[cfg(windows)]
pub fn start_print_watcher(app: tauri::AppHandle) -> Result<(), String> {
    spawn_print_watcher(app)
}

#[cfg(not(windows))]
pub fn start_print_watcher(_app: tauri::AppHandle) -> Result<(), String> {
    Ok(())
}

/// Second line of defense when `ReadDirectoryChangesW` misses fast CCC writes under `%LOCALAPPDATA%\\Temp\\CCC`.
#[cfg(windows)]
pub fn spawn_ccc_temp_poll_fallback(app: tauri::AppHandle) {
    let res = thread::Builder::new()
        .name("uce-ccc-temp-poll".into())
        .spawn(move || poll_ccc_temp_loop(app));
    if let Err(e) = res {
        eprintln!("UCE_POLL_THREAD_FAILED err={e}");
    }
}

#[cfg(windows)]
fn collect_ccc_poll_candidates(
    root: &Path,
    out: &mut Vec<PathBuf>,
    depth: usize,
    max_depth: usize,
) {
    if depth > max_depth {
        return;
    }
    if !root.exists() || !root.is_dir() {
        return;
    }
    let Ok(rd) = fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.eq_ignore_ascii_case(".uce_staging") {
                continue;
            }
            collect_ccc_poll_candidates(&p, out, depth + 1, max_depth);
        } else if p.is_file() {
            if converter::path_is_under_uce_staging(&p) {
                continue;
            }
            if extension_kind(&p).is_some() {
                out.push(p);
            }
        }
    }
}

#[cfg(windows)]
fn poll_ccc_temp_loop(app: tauri::AppHandle) {
    let root = print_config::ccc_temp_watch_path();
    eprintln!(
        "UCE_POLL_SCAN_STARTED path={} interval_secs=1 max_depth=8",
        root.display()
    );
    let mut seen: HashSet<String> = HashSet::new();
    loop {
        thread::sleep(Duration::from_secs(1));
        let _ = fs::create_dir_all(&root);
        let mut candidates = Vec::new();
        collect_ccc_poll_candidates(&root, &mut candidates, 0, 8);
        eprintln!("UCE_POLL_SCAN_COUNT files_found={}", candidates.len());
        for path in candidates {
            let fp = ccc_batch::file_fingerprint(&path).unwrap_or_else(|| {
                path.to_string_lossy().to_lowercase()
            });
            if seen.contains(&fp) {
                continue;
            }
            seen.insert(fp.clone());
            eprintln!(
                "UCE_POLL_DETECTED_FILE path={} fp={}",
                path.display(),
                fp
            );
            if print_config::ccc_temp_watch_only() {
                ccc_batch::handle_ccc_temp_file(&app, path);
            } else {
                handle_path(&app, path);
            }
        }
        if seen.len() > 8000 {
            seen.clear();
            eprintln!("UCE_POLL_SEEN_PRUNED reason=max_entries");
        }
    }
}

#[cfg(not(windows))]
pub fn spawn_ccc_temp_poll_fallback(_app: tauri::AppHandle) {}
