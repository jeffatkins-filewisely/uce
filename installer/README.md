# FileWisely Desktop OS v1 — shop installer

End-to-end deployment for:

- **CCC → virtual PDF printer** → `C:\FileWisely\Incoming\`
- **UCE (Tauri)** watches the folder, auto-PDF upload, Word→PDF via LibreOffice
- **RO panel** reads `uce-ro-status` (POST + no-store; see app `main.js`)

## FileWisely PDF printer only (Bullzip)

To prepare **Bullzip** for silent PDF capture to `C:\FileWisely\Incoming\` without using the full `install.ps1` flow, run **as Administrator**:

```powershell
powershell -ExecutionPolicy Bypass -File .\installer\setup-filewisely-printer.ps1
```

From the **`installer`** folder:

```powershell
powershell -ExecutionPolicy Bypass -File .\setup-filewisely-printer.ps1
```

Place the Bullzip installer at **`installer/pdf-printer/bullzip.exe`** for a silent install when the printer is not already present. See **`README-filewisely-printer.md`** for details and **`verify-filewisely-printer.ps1`** for a quick PASS/FAIL check.

**Printer-only EXE:** Compile **`FileWiselyInstaller.iss`** in Inno Setup (from **`installer/`**). Output **`Output/FileWiselyInstaller.exe`** — admin install, runs **`setup-filewisely-printer.ps1`** then **`verify-filewisely-printer.ps1`**. Full **`FileWisely-Setup.exe`** (`setup.iss`) already includes the same scripts; **`install.ps1`** runs **`setup-filewisely-printer.ps1`** after configuring the `PDF Writer\` Bullzip queues so **`%ProgramData%\Bullzip\PDF Printer\global.ini`** is applied on every shop install.

## Prerequisites

1. **UCE build** — copy your release build into `uce/` (see `uce/README.md`).
2. **Virtual PDF printer** (optional but recommended for **no Save dialog**) — place the Bullzip PDF Printer installer under `pdf-printer/` (see `pdf-printer/README.md`). Microsoft Print to PDF always prompts for a path unless you use another driver.
3. **LibreOffice** — for Word→PDF conversion (`soffice.exe`), if shops print/save `.doc`/`.docx` into Incoming.
4. **Run PowerShell as Administrator** when installing printer drivers.

## Install

```powershell
cd installer
Set-ExecutionPolicy -Scope Process Bypass -Force
.\install.ps1 -BusinessId "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
```

Flags:

| Flag | Meaning |
|------|--------|
| `-SkipPrinter` | Only create folders + copy UCE + config (printer already installed). |
| `-SkipUCE` | Only folders + printer hints (UCE already deployed). |
| `-Strict` | Fail install if no PDF printer or Bullzip `settings.ini` when expected (CI / QA). |

Install writes **`C:\FileWisely\install.log`** (PowerShell transcript) for troubleshooting.

**Self-healing printer:** if **FileWisely Printer** is missing after the first install pass, `install.ps1` runs **`Ensure-FileWiselyPrinter`** (re-silent-install from `pdf-printer\*.exe`, then **`Rename-Printer`** when possible). Requires admin / Print Management cmdlets.

**UCE runtime:** on startup the overlay calls **`uce_check_filewisely_printer`** (Rust → PowerShell) and logs a **console warning** if the exact printer name is missing (lists Bullzip/FileWisely/PDF matches).

## Single EXE: Inno Setup (`FileWisely-Setup.exe`)

1. Install [Inno Setup 6](https://jrsoftware.org/isinfo.php).
2. From the repo root, build UCE (`npm run tauri build` or your CI) and copy the **release** folder contents into `installer/uce/` (must include `UCE.exe` — name comes from `productName` in `src-tauri/tauri.conf.json`).
3. Optionally add the Bullzip/PDF24 installer under `installer/pdf-printer/`.
4. Edit `installer/setup.iss`: set `#define MyBusinessId` to the shop UUID (or pass `/DMyBusinessId=...` on the command line).
5. Open `installer/setup.iss` in Inno → **Build → Compile** (or run `ISCC.exe setup.iss` from `installer/`).

Output (default): `installer/Output/FileWisely-Setup.exe` (or next to the `.iss`, depending on Inno “Output” settings).

**What it does:** installs payload to `C:\FileWisely\` (`install.ps1`, `uce\`, `pdf-printer\`), runs `install.ps1` elevated (admin) to create Incoming/App, printer steps, Startup shortcut, then adds Start Menu / optional Desktop icons pointing at `uce\UCE.exe` (present before `install.ps1` runs; `install.ps1` also copies to `App\`).

**EXE name:** If your Tauri binary is not `UCE.exe`, change the `[Icons]` `Filename` lines in `setup.iss` to match.

**Wizard:** `setup.iss` sets **`DisableReadyPage`** and **`DisableFinishedPage`** for a minimal flow. Comment those out if shops need a clear “Setup finished” screen.

## After install

1. **Business ID** — UCE stores tenant ID in its own app data (see in-app tenant setup). The installer writes `C:\FileWisely\App\filewisely-desktop.json` as an **IT reference**; staff still paste UUID in UCE if not baked into your branded build.
2. **Printer name** — should match `FW_PRINTER_DISPLAY_NAME` in `src-tauri/src/config/print_config.rs` (default **FileWisely Printer**). Rename in *Settings → Bluetooth & devices → Printers & scanners* if needed.
3. **Bullzip** — verify `settings.ini` under `%APPDATA%\Bullzip\PDF Printer\` points output to `C:\FileWisely\Incoming\` and disables prompts (see `pdf-printer/bullzip-settings.example.ini`).
4. **Smoke test** — print from Notepad to **FileWisely Printer** → file lands in Incoming → UCE toast / upload (with CCC context + PDF mode).

## Checklist (before roll-out)

- [ ] Print from CCC → PDF lands in `C:\FileWisely\Incoming\` (no dialog if Bullzip configured).
- [ ] UCE running (tray / startup shortcut).
- [ ] Upload succeeds (tenant + `VITE_UCE_UPLOAD_URL` in build).
- [ ] RO panel shows **In System** after `uce-ro-status` returns data (DevTools: `uce_ro_debug` localStorage for raw JSON).

**CI:** Run `ISCC.exe` on `installer/setup.iss` after copying `uce\` and optional `pdf-printer\` artifacts so every build produces `FileWisely-Setup.exe` without manual clicks.
