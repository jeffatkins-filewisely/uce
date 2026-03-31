# Re-apply silent FileWisely output for Bullzip (run as Administrator).
# Use after install or if the "Create File" / Save As dialog reappears.
param(
    [string]$Incoming = "C:\FileWisely\Incoming"
)

$ErrorActionPreference = "Stop"
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Warning "Run this script elevated (Administrator) so C:\ProgramData\PDF Writer can be written."
    exit 1
}

if (-not (Test-Path $Incoming)) {
    New-Item -ItemType Directory -Force -Path $Incoming | Out-Null
}

$pdPdfWriter = Join-Path $env:ProgramData "PDF Writer"
$queues = @("Bullzip PDF Printer", "FileWisely Printer")
$inc = $Incoming.TrimEnd('\')
$outLine = "Output=$inc" + '\<date>_<time>_<docname>.pdf'
$iniBody = @"
[PDF Printer]
$outLine
ShowSaveAS=never
ShowSettings=never
ShowPDF=no
DisableOptionDialog=yes
ConfirmOverwrite=no
"@

foreach ($q in $queues) {
    $dir = Join-Path $pdPdfWriter $q
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }
    $dest = Join-Path $dir "global.ini"
    if (Test-Path $dest) {
        Copy-Item $dest "$dest.bak-$(Get-Date -Format yyyyMMddHHmmss)" -Force
    }
    Set-Content -Path $dest -Value $iniBody -Encoding UTF8
    Write-Host "OK: $dest"
}

Write-Host ""
Write-Host "Done. Print a test page to FileWisely Printer - it should save to $Incoming without a dialog."
Write-Host "Install LibreOffice if you need Word .doc/.docx in that folder converted to PDF automatically (UCE)."
