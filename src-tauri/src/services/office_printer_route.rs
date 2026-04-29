//! Route Office documents through **FileWisely Printer** (Word COM → spooler → PDF in Incoming),
//! matching the proven manual print path. Windows + Word required for silent print.

use crate::config::print_config;
use crate::pdf_watch_config::{OfficeIngestionMode, PdfWatchConfig};
use crate::services::converter;

use serde_json::json;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter};
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const ROUTE_TTL: Duration = Duration::from_secs(20 * 60);

static ROUTED_AT: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn routed_map() -> &'static Mutex<HashMap<String, Instant>> {
    ROUTED_AT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn prune_routed(now: Instant, m: &mut HashMap<String, Instant>) {
    m.retain(|_, t| now.duration_since(*t) < ROUTE_TTL);
}

/// True if this path was successfully sent via printer route recently (skip duplicate print/staging churn).
pub fn office_printer_recently_routed(path: &Path) -> bool {
    let key = converter::incoming_pipeline_key(path);
    let now = Instant::now();
    let Ok(mut g) = routed_map().lock() else {
        return false;
    };
    prune_routed(now, &mut g);
    g.contains_key(&key)
}

pub fn mark_office_printer_routed(path: &Path) {
    let key = converter::incoming_pipeline_key(path);
    let now = Instant::now();
    if let Ok(mut g) = routed_map().lock() {
        prune_routed(now, &mut g);
        g.insert(key, now);
    }
}

/// Result of attempting printer-first routing before LibreOffice staging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficePrinterFirstResult {
    /// Already printed via this path recently — do not print or convert again.
    SkipDuplicateWindow,
    /// Silent (or manual) print path succeeded; expect PDF via FileWisely Printer.
    HandledByPrinter,
    /// Run staging / conversion pipeline.
    FallThroughToStaging,
}

/// Printer-first path when `office_ingestion_mode` is `printer_preferred`; otherwise staging only.
pub fn route_office_document_printer_first(
    app: &AppHandle,
    path: &Path,
    cfg: &PdfWatchConfig,
    context: &str,
) -> OfficePrinterFirstResult {
    if office_printer_recently_routed(path) {
        eprintln!(
            "[UCE] OFFICE_ROUTE_DECISION not_intercepted path={} context={} reason=recent_successful_printer_route",
            path.display(),
            context
        );
        return OfficePrinterFirstResult::SkipDuplicateWindow;
    }

    if cfg.office_ingestion_mode != OfficeIngestionMode::PrinterPreferred {
        eprintln!(
            "[UCE] OFFICE_ROUTE_DECISION staging_convert path={} context={} reason=office_ingestion_mode",
            path.display(),
            context
        );
        return OfficePrinterFirstResult::FallThroughToStaging;
    }

    eprintln!(
        "[UCE] OFFICE_ROUTE_DECISION printer_preferred path={} context={}",
        path.display(),
        context
    );
    eprintln!(
        "[UCE] OFFICE_ROUTING_DETECTED path={} context={} mode=printer_preferred",
        path.display(),
        context
    );

    if cfg.office_auto_print_silent {
        eprintln!(
            "[UCE] OFFICE_ROUTING_PRINT_STARTED path={} printer={}",
            path.display(),
            print_config::FW_PRINTER_DISPLAY_NAME
        );
        match try_print_office_to_filewisely(path) {
            Ok(()) => {
                eprintln!(
                    "[UCE] OFFICE_ROUTING_PRINT_FINISHED path={}",
                    path.display()
                );
                eprintln!(
                    "[UCE] OFFICE_INGESTION_MODE=printer_preferred success path={}",
                    path.display()
                );
                mark_office_printer_routed(path);
                return OfficePrinterFirstResult::HandledByPrinter;
            }
            Err(e) => {
                eprintln!(
                    "[UCE] OFFICE_ROUTING_PRINT_FAILED path={} err={}",
                    path.display(),
                    e
                );
            }
        }
    } else {
        eprintln!(
            "[UCE] OFFICE_ROUTE_DECISION not_intercepted path={} context={} reason=silent_print_disabled",
            path.display(),
            context
        );
    }

    if cfg.office_print_prompt_fallback {
        eprintln!(
            "[UCE] OFFICE_ROUTING_FALLBACK_PROMPT path={}",
            path.display()
        );
        let path_str = path.to_string_lossy().to_string();
        let _ = app.emit(
            "uce-office-print-prompt",
            json!({
                "path": path_str,
                "message": "Send this document to FileWisely?",
                "reason": "silent_print_failed",
            }),
        );
    }

    eprintln!(
        "[UCE] OFFICE_ROUTING_FALLBACK=staging_convert path={}",
        path.display()
    );
    OfficePrinterFirstResult::FallThroughToStaging
}

fn ps_escape_single_quoted(s: &str) -> String {
    s.replace('\'', "''")
}

/// Print `.doc` / `.docx` / `.rtf` using Word.Application to **FileWisely Printer** (headless Word, no dialog).
#[cfg(windows)]
pub fn try_print_office_to_filewisely(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err("path is not a file".into());
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if !matches!(ext.as_str(), "doc" | "docx" | "rtf") {
        return Err(format!("not an Office document extension: {ext}"));
    }

    let doc_path = path.to_str().ok_or("path is not valid UTF-8")?;
    let doc_lit = ps_escape_single_quoted(doc_path);
    let printer = print_config::FW_PRINTER_DISPLAY_NAME;
    let printer_lit = ps_escape_single_quoted(printer);

    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$docPath = '{doc_lit}'
$want = '{printer_lit}'
$prn = Get-CimInstance -ClassName Win32_Printer -ErrorAction SilentlyContinue | Where-Object {{ $_.Name -eq $want }}
if (-not $prn) {{ Write-Output 'PRINTER_NOT_FOUND'; exit 2 }}
$active = "$($prn.Name) on $($prn.PortName)"
$word = $null
$doc = $null
try {{
  $word = New-Object -ComObject Word.Application
  $word.Visible = $false
  $word.DisplayAlerts = 0
  $word.ActivePrinter = $active
  $doc = $word.Documents.Open($docPath, $false, $true, $false)
  $doc.PrintOut($true)
  Start-Sleep -Seconds 2
}} finally {{
  if ($null -ne $doc) {{ try {{ $doc.Close([ref]$false) }} catch {{ }} }}
  if ($null -ne $word) {{
    try {{ $word.Quit([ref]$false) }} catch {{ }}
    [System.Runtime.InteropServices.Marshal]::ReleaseComObject($word) | Out-Null
  }}
}}
Write-Output 'OK'
exit 0
"#
    );

    let mut ps_cmd = Command::new("powershell.exe");
    ps_cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-WindowStyle",
        "Hidden",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    let out = crate::services::process_launch::run_output(
        "office_printer_route",
        "word_print_com",
        ps_cmd,
        crate::services::process_launch::TIMEOUT_DEFAULT,
    )
    .map_err(|e| format!("powershell: {e}"))?;

    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if out.status.success() && stdout.contains("OK") {
        return Ok(());
    }
    if stdout.contains("PRINTER_NOT_FOUND") || stderr.contains("PRINTER_NOT_FOUND") {
        return Err(format!("printer '{printer}' not found"));
    }
    Err(format!(
        "word_print exit={} stdout={stdout} stderr={stderr}",
        out.status
    ))
}

#[cfg(not(windows))]
pub fn try_print_office_to_filewisely(_path: &Path) -> Result<(), String> {
    Err("office printer routing is only supported on Windows".into())
}
