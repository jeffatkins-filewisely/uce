//! Runtime check for the virtual PDF printer (Windows). Used on UCE startup for self-monitoring.

use serde::{Deserialize, Serialize};
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
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", PS])
        .output()
        .map_err(|e| format!("powershell: {e}"))?;
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
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", PS])
        .output()
        .map_err(|e| format!("powershell: {e}"))?;
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

/// Re-run silent PDF printer installer from `C:\FileWisely\pdf-printer` **or** bundled `pdf-printer` in app resources (MSI),
/// then rename to **FileWisely Printer** if needed.
#[cfg(windows)]
pub fn repair_filewisely_printer(bundled_pdf_printer_dir: Option<PathBuf>) -> Result<RepairPrinterResult, String> {
    if check_filewisely_printer()
        .map(|r| r.filewisely_exact)
        .unwrap_or(false)
    {
        return Ok(RepairPrinterResult {
            ok: true,
            message: "Printer already present".into(),
        });
    }

    let shop = PathBuf::from(r"C:\FileWisely\pdf-printer");
    printer_repair_debug(format!(
        "looking under shop dir: {:?} exists={}",
        shop,
        shop.is_dir()
    ));
    if let Some(ref b) = bundled_pdf_printer_dir {
        printer_repair_debug(format!(
            "looking under bundled dir: {:?} exists={}",
            b,
            b.is_dir()
        ));
    } else {
        printer_repair_debug("bundled pdf-printer dir: (none — resource_dir unavailable?)");
    }

    let mut setup: Option<PathBuf> = find_pdf_setup_exe_in(&shop);
    if setup.is_none() {
        if let Some(ref dir) = bundled_pdf_printer_dir {
            if dir.is_dir() {
                setup = find_pdf_setup_exe_in(dir);
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

    printer_repair_debug(format!("running installer: {:?}", setup_exe));
    // Bullzip ships as Inno Setup: /SILENT is weaker; use Inno-style silent + no restart.
    let status = std::process::Command::new(&setup_exe)
        .args(["/VERYSILENT", "/NORESTART"])
        .status()
        .map_err(|e| format!("run PDF installer: {e}"))?;
    if !status.success() {
        printer_repair_debug(format!("installer exit code: {:?}", status.code()));
        return Ok(RepairPrinterResult {
            ok: false,
            message: format!(
                "PDF installer exited with code {:?}",
                status.code()
            ),
        });
    }

    std::thread::sleep(Duration::from_secs(5));
    let _ = rename_pdf_queue_to_filewisely();

    let ok = check_filewisely_printer()
        .map(|r| r.filewisely_exact)
        .unwrap_or(false);
    printer_repair_debug(format!(
        "after rename/check: FileWisely Printer present = {}",
        ok
    ));
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
pub fn repair_filewisely_printer(_bundled_pdf_printer_dir: Option<PathBuf>) -> Result<RepairPrinterResult, String> {
    Ok(RepairPrinterResult {
        ok: true,
        message: "skip".into(),
    })
}
