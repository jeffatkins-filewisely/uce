# FileWisely - prepare Bullzip PDF Printer for silent capture to Incoming (run as Administrator).
# Idempotent: safe to re-run; backs up existing global.ini before overwrite.
# Default BullzipExe is empty: script uses pdf-printer\bullzip.exe next to this file.
# Edit BullzipSilentArgs if your vendor build needs different silent flags.

param(
    [string]$IncomingRoot = "C:\FileWisely\Incoming",
    [string]$PrinterDisplayName = "FileWisely Printer",
    [string]$BullzipExe = "",
    [string[]]$BullzipSilentArgs = @("/VERYSILENT", "/NORESTART")
)

$ErrorActionPreference = "Stop"

function Write-Ok {
    param([string]$Message)
    Write-Host "[OK] $Message" -ForegroundColor Green
}

function Write-Fail {
    param([string]$Message)
    Write-Host "[FAIL] $Message" -ForegroundColor Red
}

function Write-WarnLine {
    param([string]$Message)
    Write-Host "[WARN] $Message" -ForegroundColor Yellow
}

# --- Admin required (ProgramData, printer driver / rename) ---
$principal = New-Object Security.Principal.WindowsPrincipal(
    [Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host ""
    Write-Fail "Run PowerShell as Administrator (right-click -> Run as administrator)."
    exit 1
}

$ScriptDir = $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($BullzipExe)) {
    $BullzipExe = Join-Path $ScriptDir "pdf-printer\bullzip.exe"
}

# Bullzip queue config - system-wide defaults (path per FileWisely spec)
$GlobalIniDir = Join-Path $env:ProgramData "Bullzip\PDF Printer"
$GlobalIniPath = Join-Path $GlobalIniDir "global.ini"

# Exact content required for FileWisely (do not alter keys without testing Bullzip).
# Single-quoted here-string so <date>/<time> tokens stay literal for Bullzip.
$GlobalIniContent = @'
[PDF Printer]
Output=C:\FileWisely\Incoming\<date>_<time>_<docname>.pdf
ShowSaveAS=never
ShowSettings=never
ShowPDF=no
ConfirmOverwrite=no
DisableOptionDialog=yes
OpenFolder=no
OpenPDF=no
'@

try {
    Write-Host ""
    Write-Host "=== FileWisely PDF printer setup ===" -ForegroundColor Cyan
    Write-Host ""

    # 1) Incoming folder (must match Output= path above)
    Write-Host "Folder: $IncomingRoot"
    if (-not (Test-Path -LiteralPath $IncomingRoot)) {
        New-Item -ItemType Directory -Force -Path $IncomingRoot | Out-Null
    }
    if (Test-Path -LiteralPath $IncomingRoot) {
        Write-Ok "Folder exists"
    }
    else {
        Write-Fail "Could not create folder"
        exit 1
    }

    # 2) Silent Bullzip install (only if vendor exe is present)
    if (Test-Path -LiteralPath $BullzipExe) {
        $already = Get-Printer -Name $PrinterDisplayName -ErrorAction SilentlyContinue
        if (-not $already) {
            Write-Host "Installer: $BullzipExe"
            Write-Host "Silent args: $($BullzipSilentArgs -join ' ')"
            $p = Start-Process -FilePath $BullzipExe -ArgumentList $BullzipSilentArgs -PassThru -Wait -ErrorAction Stop
            if ($null -ne $p.ExitCode -and $p.ExitCode -ne 0) {
                Write-WarnLine "Bullzip installer exit code $($p.ExitCode) - continuing (driver may still register)."
            }
            Write-Host "Waiting for spooler / driver registration..."
            Start-Sleep -Seconds 6
            Write-Ok "Bullzip silent install finished"
        }
        else {
            Write-Ok "Printer '$PrinterDisplayName' already present - skipped Bullzip installer"
        }
    }
    else {
        Write-WarnLine "No installer at '$BullzipExe' - skipped silent install (add pdf-printer\bullzip.exe)"
    }

    # 3) global.ini - backup existing, then write
    Write-Host "Config: $GlobalIniPath"
    if (-not (Test-Path -LiteralPath $GlobalIniDir)) {
        New-Item -ItemType Directory -Force -Path $GlobalIniDir | Out-Null
    }
    if (Test-Path -LiteralPath $GlobalIniPath) {
        $bak = "$GlobalIniPath.bak-$(Get-Date -Format 'yyyyMMddHHmmss')"
        Copy-Item -LiteralPath $GlobalIniPath -Destination $bak -Force
        Write-Host "  (backed up previous to $(Split-Path $bak -Leaf))"
    }
    Set-Content -LiteralPath $GlobalIniPath -Value $GlobalIniContent -Encoding UTF8
    Write-Ok "Config written"

    # 4) Rename default Bullzip queue to FileWisely Printer (if needed)
    $target = Get-Printer -Name $PrinterDisplayName -ErrorAction SilentlyContinue
    if (-not $target) {
        $candidate = Get-Printer -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match '(?i)bullzip' -and $_.Name -ne $PrinterDisplayName } |
            Select-Object -First 1
        if ($candidate) {
            Write-Host "Rename: '$($candidate.Name)' -> '$PrinterDisplayName'"
            Rename-Printer -Name $candidate.Name -NewName $PrinterDisplayName -ErrorAction Stop
            Write-Ok "Printer renamed"
        }
        else {
            Write-WarnLine "No Bullzip-named printer found to rename - install Bullzip or place bullzip.exe and re-run"
        }
    }
    else {
        Write-Ok "Printer already named '$PrinterDisplayName' - no rename needed"
    }

    # 5) Verification (child process - reliable exit codes on Windows PowerShell)
    Write-Host ""
    Write-Host "Running verification..."
    $verifyScript = Join-Path $ScriptDir "verify-filewisely-printer.ps1"
    $v = Start-Process -FilePath "powershell.exe" -ArgumentList @(
        "-NoProfile", "-ExecutionPolicy", "Bypass",
        "-File", $verifyScript,
        "-IncomingRoot", $IncomingRoot,
        "-PrinterDisplayName", $PrinterDisplayName,
        "-GlobalIniPath", $GlobalIniPath
    ) -Wait -PassThru -NoNewWindow
    if ($v.ExitCode -ne 0) {
        Write-Host ""
        Write-Fail "Verification failed (exit $($v.ExitCode))"
        exit $v.ExitCode
    }

    Write-Host ""
    Write-Ok "Setup finished successfully"
    exit 0
}
catch {
    Write-Host ""
    Write-Fail $_.Exception.Message
    if ($_.ScriptStackTrace) { Write-Host $_.ScriptStackTrace -ForegroundColor DarkGray }
    exit 1
}
