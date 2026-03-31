# FileWisely PDF printer (Bullzip)

Prepares **Bullzip PDF Printer** for FileWisely with **no Save As dialog** and output to **`C:\FileWisely\Incoming\`**.

## Prerequisites

- **Windows** with PowerShell 5.1 or later.
- **Administrator** session for `setup-filewisely-printer.ps1` (writes `ProgramData`, installs driver, may rename printer).
- Optional: place the Bullzip installer as **`installer/pdf-printer/bullzip.exe`** so setup can run a **silent** install automatically.

## What gets configured

| Item | Location |
|------|----------|
| Incoming folder | `C:\FileWisely\Incoming\` |
| Bullzip global defaults | `C:\ProgramData\Bullzip\PDF Printer\global.ini` |
| Printer display name | **FileWisely Printer** (rename from default Bullzip queue when possible) |

## Run setup (elevated)

From the **repository root**:

```powershell
powershell -ExecutionPolicy Bypass -File .\installer\setup-filewisely-printer.ps1
```

Or from **`installer`**:

```powershell
cd installer
powershell -ExecutionPolicy Bypass -File .\setup-filewisely-printer.ps1
```

**Right-click PowerShell → Run as administrator** if you are not already elevated.

## Verify (no admin required)

```powershell
powershell -ExecutionPolicy Bypass -File .\installer\verify-filewisely-printer.ps1
```

Prints **PASS** / **FAIL** per folder, config file, and printer name, plus an overall summary.

## Smoke test

1. Print from Notepad to **FileWisely Printer**.
2. Confirm a PDF appears under `C:\FileWisely\Incoming\` without prompts.

## Idempotent behavior

Re-running **setup** is safe: it ensures the folder and `global.ini`, backs up an existing `global.ini` before overwrite, skips the Bullzip **silent** installer if **FileWisely Printer** already exists, and renames the Bullzip queue only when the target name is missing.

## One EXE: Inno Setup (`FileWiselyInstaller.exe`)

1. Install [Inno Setup 6](https://jrsoftware.org/isinfo.php).
2. Place **`installer/pdf-printer/bullzip.exe`** (recommended) next to the `.iss` file.
3. From **`installer`**, compile **`FileWiselyInstaller.iss`** (Build → Compile), or run:

   ```text
   "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" FileWiselyInstaller.iss
   ```

4. Output: **`installer/Output/FileWiselyInstaller.exe`** (default). Run it elevated; it extracts scripts and `pdf-printer\` to **`C:\FileWisely\`**, runs **`setup-filewisely-printer.ps1`**, then **`verify-filewisely-printer.ps1`**.

The full desktop bundle **`FileWisely-Setup.exe`** (`setup.iss`) also ships these scripts and **`install.ps1`** runs **`setup-filewisely-printer.ps1`** after the legacy Bullzip `PDF Writer\` global.ini step so **`C:\ProgramData\Bullzip\PDF Printer\global.ini`** is applied automatically.

## Troubleshooting

- **Verify FAIL on printer:** Install Bullzip manually or add `pdf-printer\bullzip.exe` and run setup again.
- **Bullzip still shows a dialog:** Some versions also read settings under `%ProgramData%\PDF Writer\...`. The main **`install.ps1`** still writes **`PDF Writer\...`** via `Set-BullzipSilentGlobalIni`; this script adds the **`Bullzip\PDF Printer`** path. If prompts persist, check both locations or your vendor KB.
