# FileWisely Desktop OS v1 — shop installer (folders + optional Bullzip + UCE + Startup shortcut)
# Run elevated when invoked from Inno Setup (admin required for printer drivers).

param(
    [string]$BusinessId = "",
    [switch]$SkipPrinter,
    [switch]$SkipUCE,
    # When set, fail the script if the PDF printer or Bullzip settings are missing after install (CI / QA).
    [switch]$Strict
)

$ErrorActionPreference = "Stop"
$Root = $PSScriptRoot
$FwRoot = "C:\FileWisely"
$Incoming = Join-Path $FwRoot "Incoming"
$AppDir = Join-Path $FwRoot "App"
$LogPath = Join-Path $FwRoot "install.log"
$PrinterDisplayName = "FileWisely Printer"
# Bullzip PDF Printer (Inno Setup). /SILENT alone often fails silent install; match vendor flags.
$PdfPrinterSilentArgs = @("/VERYSILENT", "/NORESTART")

$TranscriptOn = $false
try {
    New-Item -ItemType Directory -Force -Path $FwRoot, $Incoming, $AppDir | Out-Null
    $watchdogSrc = Join-Path $Root "watchdog.ps1"
    if (Test-Path $watchdogSrc) {
        Copy-Item -Path $watchdogSrc -Destination (Join-Path $FwRoot "watchdog.ps1") -Force
        Write-Host "OK: watchdog.ps1 -> $FwRoot"
    }
    Start-Transcript -Path $LogPath -Force -ErrorAction Stop | Out-Null
    $TranscriptOn = $true
}
catch {
    Write-Warning "Could not start transcript at ${LogPath}: $_"
}

function Stop-InstallTranscript {
    if ($TranscriptOn) {
        try {
            Stop-Transcript | Out-Null
        }
        catch { }
        $script:TranscriptOn = $false
    }
}

function Fail-Install {
    param([string]$Message, [int]$Code = 1)
    Write-Host ""
    Write-Host "ERROR: $Message" -ForegroundColor Red
    Stop-InstallTranscript
    exit $Code
}

function Ensure-FileWiselyPrinter {
    param(
        [Parameter(Mandatory = $true)][string]$InstallerRoot,
        [Parameter(Mandatory = $true)][string]$DisplayName
    )
    $existing = Get-Printer -Name $DisplayName -ErrorAction SilentlyContinue
    if ($existing) {
        Write-Host "OK: Printer '$DisplayName' already present."
        return $true
    }
    Write-Warning "Printer '$DisplayName' missing — attempting repair (re-run installer + rename)..."
    $setup = Get-ChildItem -Path (Join-Path $InstallerRoot "pdf-printer") -Filter "*.exe" -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -notmatch '(?i)uninst|vcredist|vcruntime' } |
        Select-Object -First 1
    if (-not $setup) {
        Write-Warning "No pdf-printer\*.exe available to repair printer."
        return $false
    }
    try {
        $null = Start-Process -FilePath $setup.FullName -ArgumentList $PdfPrinterSilentArgs -Wait -PassThru -ErrorAction Stop
    }
    catch {
        Write-Warning "Re-install printer failed: $_"
        return $false
    }
    Start-Sleep -Seconds 5
    $cand = Get-Printer -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '(?i)bullzip|pdf printer' -and $_.Name -ne $DisplayName } |
        Select-Object -First 1
    if ($cand) {
        try {
            Rename-Printer -Name $cand.Name -NewName $DisplayName -ErrorAction Stop
            Write-Host "Renamed printer '$($cand.Name)' -> '$DisplayName'"
        }
        catch {
            Write-Warning "Rename-Printer failed (run Print Management as admin): $_"
        }
    }
    $final = Get-Printer -Name $DisplayName -ErrorAction SilentlyContinue
    if ($final) {
        Write-Host "OK: Printer '$DisplayName' ready after repair."
        return $true
    }
    Write-Warning "Printer '$DisplayName' still missing after repair."
    return $false
}

function Set-BullzipSilentGlobalIni {
    param(
        [Parameter(Mandatory = $true)][string]$IncomingDir
    )
    $pdPdfWriter = Join-Path $env:ProgramData "PDF Writer"
    $queues = @("Bullzip PDF Printer", "FileWisely Printer")
    $inc = $IncomingDir.TrimEnd('\')
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
            try {
                New-Item -ItemType Directory -Force -Path $dir | Out-Null
            }
            catch {
                Write-Warning "Could not create $dir : $_"
                continue
            }
        }
        $dest = Join-Path $dir "global.ini"
        try {
            if (Test-Path $dest) {
                Copy-Item $dest "$dest.bak-$(Get-Date -Format yyyyMMddHHmmss)" -Force -ErrorAction SilentlyContinue
            }
            Set-Content -Path $dest -Value $iniBody -Encoding UTF8
            Write-Host "OK: Silent Bullzip global.ini -> $dest"
        }
        catch {
            Write-Warning "Could not write $dest : $_"
        }
    }
}

try {

    Write-Host ""
    Write-Host "=== FileWisely Desktop — install ===" -ForegroundColor Cyan
    Write-Host "Log: $LogPath"
    Write-Host ""

    Write-Host "Creating folders..."
    Write-Host "  OK: $Incoming"
    Write-Host "  OK: $AppDir"

    $ranPrinterSetup = $false
    $pdfPrinters = @()

    # --- Virtual PDF printer (Bullzip or similar) ---
    if (-not $SkipPrinter) {
        $setup = Get-ChildItem -Path (Join-Path $Root "pdf-printer") -Filter "*.exe" -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -notmatch '(?i)uninst|vcredist|vcruntime' } |
            Select-Object -First 1

        if ($setup) {
            Write-Host ""
            Write-Host "Running printer installer: $($setup.FullName)"
            $ranPrinterSetup = $true
            $p = Start-Process -FilePath $setup.FullName -ArgumentList $PdfPrinterSilentArgs -PassThru -Wait
            if ($null -ne $p.ExitCode -and $p.ExitCode -ne 0) {
                Write-Warning "Installer exit code $($p.ExitCode). Check vendor docs for silent switches."
            }

            Write-Host "Waiting for spooler / driver registration..."
            Start-Sleep -Seconds 5
        }
        else {
            Write-Warning "No installer .exe under pdf-printer. Install Bullzip/PDF24 manually; set default output to: $Incoming"
        }

        $pdfPrinters = @(Get-Printer -ErrorAction SilentlyContinue | Where-Object {
                $_.Name -match '(?i)bullzip|filewisely|pdf printer'
            })
        if ($pdfPrinters.Count -eq 0) {
            $msg = "No Bullzip / FileWisely / PDF Printer detected (name should match *Bullzip*, *FileWisely*, or *PDF Printer*)."
            if ($ranPrinterSetup -or $Strict) {
                Fail-Install $msg
            }
            else {
                Write-Warning $msg
            }
        }
        else {
            Write-Host "OK: Detected printer(s): $($pdfPrinters.Name -join ', ')"
        }

        $bullIniDir = Join-Path $env:APPDATA "Bullzip\PDF Printer"
        if (-not (Test-Path $bullIniDir)) {
            New-Item -ItemType Directory -Force -Path $bullIniDir | Out-Null
        }
        $exampleIni = Join-Path $Root "pdf-printer\bullzip-settings.example.ini"
        $targetIni = Join-Path $bullIniDir "settings.ini"
        if (Test-Path $exampleIni) {
            if (-not (Test-Path $targetIni)) {
                Copy-Item $exampleIni $targetIni -Force
                Write-Host "Wrote starter Bullzip settings: $targetIni (merge tokens per KB; set Output=$Incoming)"
            }
            else {
                Write-Host "Bullzip settings.ini already exists — merge pdf-printer\bullzip-settings.example.ini manually if needed."
            }
        }

        Start-Sleep -Seconds 1
        if ($ranPrinterSetup -and -not (Test-Path $targetIni)) {
            $msg = "Bullzip settings.ini missing at $targetIni — Save dialog may appear until configured."
            if ($Strict) {
                Fail-Install $msg
            }
            else {
                Write-Warning $msg
            }
        }
        elseif (Test-Path $targetIni) {
            Write-Host "OK: Printer config present: $targetIni"
        }

        if (-not (Get-Printer -Name $PrinterDisplayName -ErrorAction SilentlyContinue)) {
            [void](Ensure-FileWiselyPrinter -InstallerRoot $Root -DisplayName $PrinterDisplayName)
        }
        $pdfPrinters = @(Get-Printer -ErrorAction SilentlyContinue | Where-Object {
                $_.Name -match '(?i)bullzip|filewisely|pdf printer'
            })

        # Prefer default printer = FileWisely pipeline (rename in Windows if installer left vendor name).
        $defaultTarget = $null
        if (Get-Printer -Name $PrinterDisplayName -ErrorAction SilentlyContinue) {
            $defaultTarget = $PrinterDisplayName
        }
        elseif ($pdfPrinters -and $pdfPrinters.Count -gt 0) {
            $defaultTarget = $pdfPrinters[0].Name
            Write-Warning "Printer named '$PrinterDisplayName' not found — setting default to '$defaultTarget'. Rename printer to '$PrinterDisplayName' for consistency with UCE docs."
        }
        if ($defaultTarget) {
            try {
                Set-Printer -Name $defaultTarget -IsDefault $true -ErrorAction Stop
                Write-Host "Default printer set to: $defaultTarget"
            }
            catch {
                Write-Warning "Could not set default printer (${defaultTarget}): $_"
            }
        }

        Write-Host ""
        Write-Host "Bullzip: configuring silent PDF output to $Incoming (no Save As dialog)..."
        try {
            Set-BullzipSilentGlobalIni -IncomingDir $Incoming
        }
        catch {
            Write-Warning "Bullzip silent global.ini: $_"
        }

        # Also apply FileWisely ProgramData\Bullzip\PDF Printer\global.ini + verify (same as standalone setup-filewisely-printer.ps1).
        $fwSetup = Join-Path $Root "setup-filewisely-printer.ps1"
        if (Test-Path -LiteralPath $fwSetup) {
            Write-Host ""
            Write-Host "FileWisely: running setup-filewisely-printer.ps1 (Bullzip global.ini under ProgramData\Bullzip, verification)..."
            $fwProc = Start-Process -FilePath "powershell.exe" -ArgumentList @(
                "-NoProfile", "-ExecutionPolicy", "Bypass",
                "-File", $fwSetup
            ) -Wait -PassThru -NoNewWindow
            if ($fwProc.ExitCode -ne 0) {
                if ($Strict) {
                    Fail-Install "setup-filewisely-printer.ps1 failed (exit $($fwProc.ExitCode))."
                }
                else {
                    Write-Warning "setup-filewisely-printer.ps1 exited $($fwProc.ExitCode) — verify printer and C:\ProgramData\Bullzip\PDF Printer\global.ini"
                }
            }
        }
    }

    # --- UCE (Tauri build output) ---
    if (-not $SkipUCE) {
        $uceSrc = Join-Path $Root "uce"
        if (-not (Test-Path $uceSrc)) {
            Write-Warning "Missing folder: $uceSrc"
        }
        else {
            $hasExe = @(Get-ChildItem $uceSrc -Filter "*.exe" -Recurse -ErrorAction SilentlyContinue)
            if ($hasExe.Count -eq 0) {
                Write-Warning "No .exe under uce — copy your Tauri release build here, then re-run with -SkipPrinter if printer is already done."
            }
            else {
                Copy-Item -Path (Join-Path $uceSrc "*") -Destination $AppDir -Recurse -Force
                Write-Host ""
                Write-Host "UCE copied to $AppDir"
            }
        }
    }

    # --- IT reference JSON (UCE still uses in-app tenant / build-time env) ---
    $bid = if ($BusinessId.Trim()) { $BusinessId.Trim() } else { "REPLACE_ME" }
    $configObj = [ordered]@{
        business_id     = $bid
        deployed_at     = (Get-Date).ToString("o")
        incoming_folder = $Incoming
        printer_name    = $PrinterDisplayName
        notes           = "UCE stores tenant in app data after user pastes UUID (gear), or from uce-tenant.json / VITE_UCE_BUSINESS_ID in a branded build."
    }
    $cfgPath = Join-Path $AppDir "filewisely-desktop.json"
    $configObj | ConvertTo-Json -Depth 5 | Set-Content -Path $cfgPath -Encoding UTF8
    Write-Host ""
    Write-Host "Wrote reference config: $cfgPath"

    # --- Machine-local PDF watch seed (merged in UCE with per-user uce-pdf-watch.json) ---
    $seedPs1 = Join-Path $Root "seed-uce-pdf-watch.ps1"
    if (Test-Path -LiteralPath $seedPs1) {
        try {
            & $seedPs1 -OutPath (Join-Path $AppDir "uce-pdf-watch.seed.json")
        }
        catch {
            Write-Warning "seed-uce-pdf-watch.ps1 failed: $_"
        }
    }

    # --- Startup shortcut ---
    $exe = Get-ChildItem $AppDir -Filter "*.exe" -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '(?i)(uce|filewisely|sidekick|ccc)' } |
        Select-Object -First 1
    if (-not $exe) {
        $exe = Get-ChildItem $AppDir -Filter "*.exe" -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
    }
    if ($exe) {
        $startup = [Environment]::GetFolderPath("Startup")
        $lnkPath = Join-Path $startup "FileWisely UCE.lnk"
        $wsh = New-Object -ComObject WScript.Shell
        $sc = $wsh.CreateShortcut($lnkPath)
        $sc.TargetPath = $exe.FullName
        $sc.WorkingDirectory = $exe.DirectoryName
        $sc.Description = "FileWisely Universal Capture Engine"
        $sc.Save()
        Write-Host "Startup shortcut: $lnkPath -> $($exe.Name)"

        $watchdogPs1 = Join-Path $FwRoot "watchdog.ps1"
        if (Test-Path $watchdogPs1) {
            $wdLnk = Join-Path $startup "FileWisely UCE Watchdog.lnk"
            $scw = $wsh.CreateShortcut($wdLnk)
            $scw.TargetPath = "powershell.exe"
            $scw.Arguments = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$watchdogPs1`""
            $scw.WorkingDirectory = $FwRoot
            $scw.Description = "Restarts FileWisely UCE if the process stops"
            $scw.Save()
            Write-Host "Startup shortcut: $wdLnk -> watchdog.ps1"
        }
    }
    else {
        Write-Warning "No UCE .exe in $AppDir — Startup shortcut not created."
    }

    Write-Host ""
    Write-Host "=== Install complete ===" -ForegroundColor Green
    Write-Host "Checklist: (1) Printer output -> $Incoming (2) Printer display name '$PrinterDisplayName' (3) UCE tenant UUID (4) Print test from CCC"
    Write-Host ""
}
catch {
    Write-Host ""
    Write-Host "FATAL: $_" -ForegroundColor Red
    Write-Host $_.ScriptStackTrace
    Stop-InstallTranscript
    exit 1
}
finally {
    Stop-InstallTranscript
}
