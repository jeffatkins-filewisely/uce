//! Runtime check for the virtual PDF printer (Windows). Used on UCE startup for self-monitoring.

use serde::{Deserialize, Serialize};

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
    const PS: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$exact = $null -ne (Get-Printer -Name 'FileWisely Printer' -ErrorAction SilentlyContinue)
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

/// Re-run silent PDF printer installer from `C:\FileWisely\pdf-printer` and rename to **FileWisely Printer** if needed.
/// Called from JS on a cooldown — not a full `install.ps1` (avoids re-copying UCE every 30s).
#[cfg(windows)]
pub fn repair_filewisely_printer() -> Result<RepairPrinterResult, String> {
    const PS: &str = r#"
$ErrorActionPreference = 'Stop'
$DisplayName = 'FileWisely Printer'
$Root = 'C:\FileWisely'
if (Get-Printer -Name $DisplayName -ErrorAction SilentlyContinue) {
  @{ ok = $true; message = 'Printer already present' } | ConvertTo-Json -Compress
  exit 0
}
$setup = Get-ChildItem -Path (Join-Path $Root 'pdf-printer') -Filter '*.exe' -Recurse -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -notmatch '(?i)uninst|vcredist|vcruntime' } | Select-Object -First 1
if (-not $setup) {
  @{ ok = $false; message = 'No pdf-printer\*.exe under C:\FileWisely\pdf-printer' } | ConvertTo-Json -Compress
  exit 0
}
Start-Process -FilePath $setup.FullName -ArgumentList @('/SILENT') -Wait -PassThru | Out-Null
Start-Sleep -Seconds 5
$cand = Get-Printer -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -match '(?i)bullzip|pdf printer' -and $_.Name -ne $DisplayName } |
  Select-Object -First 1
if ($cand) {
  try { Rename-Printer -Name $cand.Name -NewName $DisplayName -ErrorAction Stop } catch {}
}
$ok = $null -ne (Get-Printer -Name $DisplayName -ErrorAction SilentlyContinue)
$msg = if ($ok) { 'Repair completed' } else { 'Repair attempted; rename or install printer manually' }
@{ ok = $ok; message = $msg } | ConvertTo-Json -Compress
"#;
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", PS])
        .output()
        .map_err(|e| format!("powershell: {e}"))?;
    let s = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stderr.trim().is_empty() && s.trim().is_empty() {
        return Err(format!("repair stderr: {stderr}"));
    }
    serde_json::from_str(s.trim()).map_err(|e| format!("parse repair JSON: {e}: {s} {stderr}"))
}

#[cfg(not(windows))]
pub fn repair_filewisely_printer() -> Result<RepairPrinterResult, String> {
    Ok(RepairPrinterResult {
        ok: true,
        message: "skip".into(),
    })
}
