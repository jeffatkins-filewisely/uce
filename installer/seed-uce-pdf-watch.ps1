# Writes machine-local UCE PDF watch seed (merged with per-user uce-pdf-watch.json at runtime).
# Safe to re-run; default skips if the seed file already exists (use -Force to overwrite).
# Discovery mirrors Rust pdf_watch_config (existing dirs only).

param(
    [string]$OutPath = "C:\FileWisely\App\uce-pdf-watch.seed.json",
    [switch]$Force
)

$ErrorActionPreference = "Stop"

function Add-IfExists {
    param(
        [Parameter(Mandatory = $true)][System.Collections.ArrayList]$List,
        [Parameter(Mandatory = $true)][hashtable]$Seen,
        [Parameter(Mandatory = $true)][string]$LiteralPath
    )
    if ([string]::IsNullOrWhiteSpace($LiteralPath)) { return }
    if (-not (Test-Path -LiteralPath $LiteralPath)) { return }
    try {
        $full = (Resolve-Path -LiteralPath $LiteralPath).Path
    }
    catch {
        return
    }
    $k = $full.ToLowerInvariant()
    if ($Seen.ContainsKey($k)) { return }
    $Seen[$k] = $true
    [void]$List.Add($full)
}

function Add-FirstLevelChildren {
    param(
        [Parameter(Mandatory = $true)][System.Collections.ArrayList]$List,
        [Parameter(Mandatory = $true)][hashtable]$Seen,
        [Parameter(Mandatory = $true)][string]$Parent,
        [string[]]$SkipNames = @("logs", "log", "temp", "tmp", "cache", "installer")
    )
    if (-not (Test-Path -LiteralPath $Parent)) { return }
    $skip = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($s in $SkipNames) { [void]$skip.Add($s) }
    Get-ChildItem -LiteralPath $Parent -Directory -ErrorAction SilentlyContinue | ForEach-Object {
        if ($skip.Contains($_.Name)) { return }
        Add-IfExists -List $List -Seen $Seen -LiteralPath $_.FullName
    }
}

$dirs = [System.Collections.ArrayList]::new()
$seen = @{}

if ($env:USERPROFILE) {
    $b = $env:USERPROFILE
    Add-IfExists -List $dirs -Seen $seen -LiteralPath (Join-Path $b "Downloads")
    Add-IfExists -List $dirs -Seen $seen -LiteralPath (Join-Path $b "Desktop")
    Add-IfExists -List $dirs -Seen $seen -LiteralPath (Join-Path $b "Documents")
    Add-IfExists -List $dirs -Seen $seen -LiteralPath (Join-Path $b "OneDrive\Desktop")
    Add-IfExists -List $dirs -Seen $seen -LiteralPath (Join-Path $b "OneDrive\Documents")
}

if ($env:ProgramData) {
    $cccis = Join-Path $env:ProgramData "CCCInformation Services"
    Add-IfExists -List $dirs -Seen $seen -LiteralPath $cccis
    Add-IfExists -List $dirs -Seen $seen -LiteralPath (Join-Path $cccis "CCCONE")
    Add-FirstLevelChildren -List $dirs -Seen $seen -Parent $cccis
}

Add-IfExists -List $dirs -Seen $seen -LiteralPath "C:\CCC\WORKFILES"
Add-IfExists -List $dirs -Seen $seen -LiteralPath "C:\CCC"
Add-FirstLevelChildren -List $dirs -Seen $seen -Parent "C:\CCC" -SkipNames @()

if ($env:LOCALAPPDATA) {
    Add-IfExists -List $dirs -Seen $seen -LiteralPath (Join-Path $env:LOCALAPPDATA "Temp\CCC")
}

Add-IfExists -List $dirs -Seen $seen -LiteralPath "C:\FileWisely\Incoming"

$parent = Split-Path -Parent $OutPath
if (-not (Test-Path -LiteralPath $parent)) {
    New-Item -ItemType Directory -Force -Path $parent | Out-Null
}

if ((Test-Path -LiteralPath $OutPath) -and -not $Force) {
    Write-Host "[seed-uce-pdf-watch] Exists (skip): $OutPath  (use -Force to overwrite)"
    exit 0
}

$obj = [ordered]@{
    extra_dirs                  = @($dirs)
    office_intercept_extra_dirs = @()
}
$obj | ConvertTo-Json -Depth 6 | Set-Content -Path $OutPath -Encoding UTF8
Write-Host "[seed-uce-pdf-watch] Wrote $($dirs.Count) dirs -> $OutPath"
