# Release build with Tauri updater signing (MSI + .sig).
# Requires: .tauri\uce-signing.key (pair for pubkey in src-tauri\tauri.conf.json)
# Usage: powershell -ExecutionPolicy Bypass -File .\scripts\build-release.ps1

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")
$key = Join-Path $PWD ".tauri\uce-signing.key"
if (-not (Test-Path $key)) {
  Write-Error "Missing $key — generate: npx @tauri-apps/cli@2 signer generate -w .tauri\uce-signing.key --ci"
}
$env:TAURI_SIGNING_PRIVATE_KEY = [IO.File]::ReadAllText($key).Trim()
npm run tauri build
