# Shared Bullzip / FileWisely silent PDF printer INI (no Save As, no open-after-print).
# Dot-source from install.ps1, setup-filewisely-printer.ps1, repair-bullzip-silent.ps1.

function Get-FileWiselyBullzipIniBody {
    param(
        [Parameter(Mandatory = $true)][string]$IncomingDir
    )
    $inc = $IncomingDir.TrimEnd('\')
    $outLine = "Output=$inc" + '\<date>_<time>_<docname>.pdf'
    return @"
[PDF Printer]
$outLine
ShowSaveAS=never
ShowSettings=never
ShowPDF=no
ConfirmOverwrite=no
DisableOptionDialog=yes
OpenFolder=no
OpenPDF=no
"@
}

function Write-FileWiselyBullzipIniFile {
    param(
        [Parameter(Mandatory = $true)][string]$DestPath,
        [Parameter(Mandatory = $true)][string]$Body
    )
    $dir = Split-Path -Parent $DestPath
    if (-not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }
    if (Test-Path -LiteralPath $DestPath) {
        Copy-Item -LiteralPath $DestPath -Destination "$DestPath.bak-$(Get-Date -Format yyyyMMddHHmmss)" -Force -ErrorAction SilentlyContinue
    }
    Set-Content -LiteralPath $DestPath -Value $Body -Encoding UTF8
}

function Set-FileWiselyBullzipSilentIni {
    param(
        [Parameter(Mandatory = $true)][string]$IncomingDir
    )
    $body = Get-FileWiselyBullzipIniBody -IncomingDir $IncomingDir

    # Bullzip 11+ queue paths
    $pdPdfWriter = Join-Path $env:ProgramData "PDF Writer"
    foreach ($q in @("Bullzip PDF Printer", "FileWisely Printer")) {
        $dest = Join-Path (Join-Path $pdPdfWriter $q) "global.ini"
        try {
            Write-FileWiselyBullzipIniFile -DestPath $dest -Body $body
            Write-Host "OK: Silent Bullzip global.ini -> $dest"
        }
        catch {
            Write-Warning "Could not write $dest : $_"
        }
    }

    # Legacy ProgramData path (some builds still read this)
    $legacy = Join-Path $env:ProgramData "Bullzip\PDF Printer\global.ini"
    try {
        Write-FileWiselyBullzipIniFile -DestPath $legacy -Body $body
        Write-Host "OK: Silent Bullzip global.ini -> $legacy"
    }
    catch {
        Write-Warning "Could not write $legacy : $_"
    }

    # Per-user override — often where "open file after printing" sticks
    $userIni = Join-Path $env:APPDATA "Bullzip\PDF Printer\settings.ini"
    try {
        Write-FileWiselyBullzipIniFile -DestPath $userIni -Body $body
        Write-Host "OK: Silent Bullzip settings.ini -> $userIni"
    }
    catch {
        Write-Warning "Could not write $userIni : $_"
    }
}

function Test-FileWiselyBullzipIniContent {
    param(
        [Parameter(Mandatory = $true)][string]$Path
    )
    if (-not (Test-Path -LiteralPath $Path)) {
        return $false
    }
    $text = Get-Content -LiteralPath $Path -Raw -ErrorAction SilentlyContinue
    if (-not $text) { return $false }
    $required = @(
        'ShowPDF=no',
        'OpenPDF=no',
        'OpenFolder=no',
        'ShowSaveAS=never',
        'ShowSettings=never'
    )
    foreach ($key in $required) {
        if ($text -notmatch [regex]::Escape($key)) {
            return $false
        }
    }
    return $true
}
