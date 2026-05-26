# Re-apply silent FileWisely output for Bullzip (run as Administrator).
# Use after install or if Save As / "open file after printing" reappears.
param(
    [string]$Incoming = "C:\FileWisely\Incoming"
)

$ErrorActionPreference = "Stop"
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Warning "Run this script elevated (Administrator) so ProgramData INI files can be written."
    exit 1
}

$ScriptDir = $PSScriptRoot
. (Join-Path $ScriptDir "bullzip-silent-ini.ps1")

if (-not (Test-Path $Incoming)) {
    New-Item -ItemType Directory -Force -Path $Incoming | Out-Null
}

Set-FileWiselyBullzipSilentIni -IncomingDir $Incoming

Write-Host ""
Write-Host "Done. Print a test page to FileWisely Printer - PDF should land in $Incoming with no dialog and no viewer."
