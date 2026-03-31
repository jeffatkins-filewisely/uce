# Verify FileWisely PDF printer prerequisites (folder, global.ini, printer name). No admin required for read-only checks.

param(
    [string]$IncomingRoot = "C:\FileWisely\Incoming",
    [string]$PrinterDisplayName = "FileWisely Printer",
    [string]$GlobalIniPath = ""
)

$ErrorActionPreference = "Continue"

if ([string]::IsNullOrWhiteSpace($GlobalIniPath)) {
    $GlobalIniPath = Join-Path $env:ProgramData "Bullzip\PDF Printer\global.ini"
}

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

# --- global.ini ---
Write-Host "Checking config: $GlobalIniPath"
if (Test-Path -LiteralPath $GlobalIniPath) {
    $results.Config = $true
    Write-Ok "global.ini exists"
}
else {
    Write-Fail "global.ini missing"
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
