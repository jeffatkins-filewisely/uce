# FileWisely UCE watchdog — restarts the desktop app if the process exits.
# Installed to C:\FileWisely\watchdog.ps1; optional Startup shortcut runs this hidden.

$ErrorActionPreference = "SilentlyContinue"
$ExePath = "C:\FileWisely\App\UCE.exe"

while ($true) {
    $uce = Get-Process -Name "UCE" -ErrorAction SilentlyContinue
    if (-not $uce) {
        $stamp = Get-Date -Format "o"
        Write-Host "$stamp UCE not running — restarting..."
        if (Test-Path $ExePath) {
            $dir = Split-Path -Parent $ExePath
            Start-Process -FilePath $ExePath -WorkingDirectory $dir
        }
        else {
            Write-Warning "UCE executable missing: $ExePath"
        }
    }
    Start-Sleep -Seconds 10
}
