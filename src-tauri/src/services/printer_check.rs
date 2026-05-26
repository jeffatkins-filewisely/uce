//! Runtime check for the virtual PDF printer (Windows). Used on UCE startup for self-monitoring.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrinterCheckResult {
    /// `true` if a printer named exactly `FileWisely Printer` exists.
    pub filewisely_exact: bool,
    /// Other PDF virtual printers that might be renamed (Bullzip, etc.).
    #[serde(default)]
    pub matching_names: Vec<String>,
}

#[cfg(windows)]
pub fn check_filewisely_printer() -> Result<PrinterCheckResult, String> {
    /// Must match `crate::config::print_config::FW_PRINTER_DISPLAY_NAME` (Word/COM prints to this queue).
    const PS: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$want = 'FileWisely Printer'
# `Get-Printer -Name` can be finicky; enumerate and match case-insensitively (same as Settings list).
$exact = $false
foreach ($p in @(Get-Printer -ErrorAction SilentlyContinue)) {
  if ($p.Name.Trim() -ieq $want) { $exact = $true; break }
}
$names = @(Get-Printer -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -match '(?i)bullzip|filewisely|pdf printer' } |
  ForEach-Object { $_.Name })
@{ filewisely_exact = $exact; matching_names = @($names) } | ConvertTo-Json -Compress -Depth 5
"#;
    let mut c = std::process::Command::new("powershell.exe");
    c.args(["-NoProfile", "-NonInteractive", "-Command", PS]);
    let out = super::process_launch::run_output(
        "printer_check",
        "enumerate_printers_json",
        c,
        super::process_launch::TIMEOUT_DEFAULT,
    )?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("printer check failed: {err}"));
    }
    let s = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(s.trim()).map_err(|e| format!("parse printer JSON: {e}: {s}"))
}

#[cfg(not(windows))]
pub fn check_filewisely_printer() -> Result<PrinterCheckResult, String> {
    Ok(PrinterCheckResult {
        filewisely_exact: true,
        matching_names: vec![],
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairPrinterResult {
    pub ok: bool,
    pub message: String,
}

#[cfg(windows)]
fn is_skipped_setup_exe(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("uninst") || n.contains("vcredist") || n.contains("vcruntime")
}

/// First suitable `.exe` under `root` (recursive). Skips uninstallers / VC redistributables.
#[cfg(windows)]
fn find_pdf_setup_exe_in(root: &Path) -> Option<PathBuf> {
    let read = std::fs::read_dir(root).ok()?;
    for entry in read.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = find_pdf_setup_exe_in(&p) {
                return Some(found);
            }
            continue;
        }
        let name = p.file_name()?.to_string_lossy();
        if !name.to_lowercase().ends_with(".exe") {
            continue;
        }
        if is_skipped_setup_exe(&name) {
            continue;
        }
        return Some(p);
    }
    None
}

#[cfg(windows)]
fn rename_pdf_queue_to_filewisely() -> Result<(), String> {
    const PS: &str = r#"
$ErrorActionPreference = 'Stop'
$DisplayName = 'FileWisely Printer'
$cand = Get-Printer -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -match '(?i)bullzip|pdf printer' -and $_.Name -ne $DisplayName } |
  Select-Object -First 1
if ($cand) {
  try { Rename-Printer -Name $cand.Name -NewName $DisplayName -ErrorAction Stop } catch {}
}
"#;
    let mut c = std::process::Command::new("powershell.exe");
    c.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        PS,
    ]);
    let out = super::process_launch::run_output(
        "printer_check",
        "rename_pdf_queue",
        c,
        super::process_launch::TIMEOUT_DEFAULT,
    )?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    Ok(())
}

/// Set `UCE_DEBUG_PRINTER_REPAIR=1` to print path resolution to stderr (run UCE from a console to see).
#[cfg(windows)]
fn printer_repair_debug(msg: impl AsRef<str>) {
    if std::env::var("UCE_DEBUG_PRINTER_REPAIR").ok().as_deref() != Some("1") {
        return;
    }
    eprintln!("[UCE printer repair] {}", msg.as_ref());
}

/// Modal dialog so users always see the next step (WebView toast can be clipped at 38×38).
#[cfg(windows)]
fn message_box_printer_repair_uac_hint() {
    let msg = "UCE — Install PDF printer\r\n\r\nUCE will install the FileWisely PDF printer driver.\r\n\r\n\
If Windows shows User Account Control (UAC), click YES to allow the install.\r\n\r\n\
Click OK to continue.";
    super::native_message_box::uce_show_native_dialog_flags(
        "printer_repair_uac",
        "message_box_printer_repair_uac_hint",
        msg,
        super::native_message_box::UCE_MB_INFO_FOREGROUND,
    );
}

/// Run Bullzip/Inno setup **elevated**. Printer drivers require admin; spawning `setup.exe` from a normal
/// user session fails silently without UAC. Uses `Start-Process -Verb RunAs` (one UAC prompt).
#[cfg(windows)]
fn run_pdf_setup_elevated(setup_exe: &Path) -> Result<std::process::ExitStatus, String> {
    let path_lit = setup_exe.to_string_lossy().replace('\'', "''");
    let script = format!(
        r#"$ErrorActionPreference = 'Stop'
try {{
  $p = Start-Process -FilePath '{path_lit}' -ArgumentList '/VERYSILENT','/NORESTART' -Verb RunAs -Wait -PassThru
  if ($null -eq $p) {{ exit 1 }}
  exit [int]($p.ExitCode)
}} catch {{
  exit 1
}}"#
    );
    let mut c = std::process::Command::new("powershell.exe");
    // Do not use -NonInteractive: UAC / elevation may require a logged-in desktop session.
    c.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    let out = super::process_launch::run_output(
        "printer_check",
        "elevated_pdf_setup",
        c,
        super::process_launch::TIMEOUT_UAC_ASSIST,
    )?;
    Ok(out.status)
}

#[cfg(windows)]
fn dedupe_search_roots(mut roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    roots.retain(|p| seen.insert(p.to_string_lossy().to_lowercase()));
    roots
}

/// Re-run silent PDF printer installer from known locations (shop folder, MSI resources, next to exe),
/// then rename to **FileWisely Printer** if needed.
#[cfg(windows)]
pub fn repair_filewisely_printer(search_roots: Vec<PathBuf>) -> Result<RepairPrinterResult, String> {
    if check_filewisely_printer()
        .map(|r| r.filewisely_exact)
        .unwrap_or(false)
    {
        super::printer_bullzip_ini::ensure_silent_bullzip_ini();
        return Ok(RepairPrinterResult {
            ok: true,
            message: "Printer already present".into(),
        });
    }

    let roots = dedupe_search_roots(search_roots);
    for r in &roots {
        printer_repair_debug(format!("search root {:?} exists={}", r, r.is_dir()));
    }

    let mut setup: Option<PathBuf> = None;
    for root in &roots {
        if root.is_dir() {
            if let Some(p) = find_pdf_setup_exe_in(root) {
                setup = Some(p);
                break;
            }
        }
    }

    let Some(setup_exe) = setup else {
        printer_repair_debug("no suitable .exe found (need vendor installer, not only .ini)");
        return Ok(RepairPrinterResult {
            ok: false,
            message: "No PDF printer installer found. Add Bullzip (or similar) setup.exe under C:\\FileWisely\\pdf-printer, or place it in installer\\pdf-printer before building the MSI so it bundles into the app.".into(),
        });
    };

    eprintln!(
        "[UCE] printer repair: running elevated installer: {}",
        setup_exe.display()
    );
    printer_repair_debug(format!("running installer (elevated): {:?}", setup_exe));
    message_box_printer_repair_uac_hint();
    let status = run_pdf_setup_elevated(&setup_exe)?;
    if !status.success() {
        printer_repair_debug(format!("installer exit code: {:?}", status.code()));
        return Ok(RepairPrinterResult {
            ok: false,
            message: format!(
                "PDF installer exited with code {:?} (declined UAC or silent failure — try running UCE as Administrator once, or install Bullzip manually)",
                status.code()
            ),
        });
    }

    std::thread::sleep(Duration::from_secs(8));
    let _ = rename_pdf_queue_to_filewisely();

    let ok = check_filewisely_printer()
        .map(|r| r.filewisely_exact)
        .unwrap_or(false);
    printer_repair_debug(format!(
        "after rename/check: FileWisely Printer present = {}",
        ok
    ));
    super::printer_bullzip_ini::ensure_silent_bullzip_ini();
    let msg = if ok {
        "Repair completed"
    } else {
        "Repair attempted; rename printer to FileWisely Printer in Windows Settings or re-run the shop installer"
    };
    Ok(RepairPrinterResult {
        ok,
        message: msg.into(),
    })
}

#[cfg(not(windows))]
pub fn repair_filewisely_printer(_search_roots: Vec<PathBuf>) -> Result<RepairPrinterResult, String> {
    Ok(RepairPrinterResult {
        ok: true,
        message: "skip".into(),
    })
}
