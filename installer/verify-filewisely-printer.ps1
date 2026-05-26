# Verify FileWisely PDF printer prerequisites (folder, silent INI keys, printer name).

param(
    [string]$IncomingRoot = "C:\FileWisely\Incoming",
    [string]$PrinterDisplayName = "FileWisely Printer",
    [string]$GlobalIniPath = ""
)

$ErrorActionPreference = "Continue"
$ScriptDir = $PSScriptRoot
. (Join-Path $ScriptDir "bullzip-silent-ini.ps1")

if ([string]::IsNullOrWhiteSpace($GlobalIniPath)) {
    $GlobalIniPath = Join-Path $env:ProgramData "Bullzip\PDF Printer\global.ini"
}

$iniPaths = @(
    $GlobalIniPath,
    (Join-Path $env:ProgramData "PDF Writer\FileWisely Printer\global.ini"),
    (Join-Path $env:ProgramData "PDF Writer\Bullzip PDF Printer\global.ini"),
    (Join-Path $env:APPDATA "Bullzip\PDF Printer\settings.ini")
)

$results = @{
    Folder = $false
    Config = $false
    Printer = $false
}

function Write-Ok {
    param([string]$Message)
    Write-Host "[OK] $Message" -ForegroundColor Green
}

function Write-Fail {
    param([string]$Message)
    Write-Host "[FAIL] $Message" -ForegroundColor Red
}

# --- Folder ---
Write-Host "Checking folder: $IncomingRoot"
if (Test-Path -LiteralPath $IncomingRoot) {
    $results.Folder = $true
    Write-Ok "Folder exists"
}
else {
    Write-Fail "Folder missing"
}

# --- Silent INI (at least one path must have full silent keys) ---
Write-Host "Checking silent INI (ShowPDF=no, OpenPDF=no, OpenFolder=no, no Save As)..."
$configOk = $false
foreach ($p in $iniPaths) {
    if (Test-Path -LiteralPath $p) {
        if (Test-FileWiselyBullzipIniContent -Path $p) {
            Write-Ok "Silent keys present: $p"
            $configOk = $true
        }
        else {
            Write-Fail "INI exists but missing silent keys: $p"
        }
    }
    else {
        Write-Host "  (skip missing) $p"
    }
}
$results.Config = $configOk
if (-not $configOk) {
    Write-Fail "No INI with required silent keys (run setup-filewisely-printer.ps1 elevated)"
}

# --- Printer display name ---
Write-Host "Checking printer: $PrinterDisplayName"
$pr = Get-Printer -Name $PrinterDisplayName -ErrorAction SilentlyContinue
if ($pr) {
    $results.Printer = $true
    Write-Ok "Printer present"
}
else {
    Write-Fail "Printer not found"
}

# --- Summary ---
Write-Host ""
Write-Host "========== SUMMARY =========="
$all = $results.Folder -and $results.Config -and $results.Printer
if ($all) {
    Write-Host "OVERALL: PASS" -ForegroundColor Green
    exit 0
}
Write-Host "OVERALL: FAIL" -ForegroundColor Red
Write-Host "  Folder:  $(if ($results.Folder) { 'PASS' } else { 'FAIL' })"
Write-Host "  Config:  $(if ($results.Config) { 'PASS' } else { 'FAIL' })"
Write-Host "  Printer: $(if ($results.Printer) { 'PASS' } else { 'FAIL' })"
exit 1
