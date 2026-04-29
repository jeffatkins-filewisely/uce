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

/// Modal dialog so users always see the next step (WebView toast can be clipped at 38×38).
#[cfg(windows)]
fn message_box_printer_repair_uac_hint() {
    if super::popup_suppression::guard_native_message_box(
        "native_message_box",
        "printer_repair_uac",
        "Install PDF printer (UAC hint)",
    ) {
        return;
    }
    eprintln!("UCE_UI_NATIVE_ALERT kind=printer_repair_uac title=\"UCE — Install PDF printer\"");
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;
    const MB_OK: u32 = 0;
    const MB_ICONINFORMATION: u32 = 0x40;
    const MB_SETFOREGROUND: u32 = 0x0001_0000;
    let title: Vec<u16> = OsStr::new("UCE — Install PDF printer")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let body = "UCE will install the FileWisely PDF printer driver.\r\n\r\n\
If Windows shows User Account Control (UAC), click YES to allow the install.\r\n\r\n\
Click OK to continue.";
    let text: Vec<u16> = OsStr::new(body)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
        );
    }
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
    std::process::Command::new("powershell.exe")
        // Do not use -NonInteractive: UAC / elevation may require a logged-in desktop session.
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .status()
        .map_err(|e| format!("elevated PDF installer: {e}"))
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
