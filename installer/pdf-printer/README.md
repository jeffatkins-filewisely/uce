# Virtual PDF printer payload

## Recommended: Bullzip PDF Printer

1. Download the **Bullzip PDF Printer** installer from [bullzip.com](https://www.bullzip.com) (or your approved vendor mirror).
2. Place the **`.exe` installer** in this folder (any subfolder is fine — `install.ps1` searches recursively for `*.exe`).

Silent install flags **vary by version**. Common patterns:

- `/SILENT` or `/VERYSILENT`
- `/NORESTART`

Run the installer once on a test VM and capture the correct `/help` or vendor docs before rolling out.

## Silent output (no “Create File” / Save As) — required for shops

Bullzip **11+ / 2026** stores system defaults in:

`C:\ProgramData\PDF Writer\Bullzip PDF Printer\global.ini`

After you **rename** the printer to **FileWisely Printer**, Windows may also use:

`C:\ProgramData\PDF Writer\FileWisely Printer\global.ini`

**`install.ps1`** (elevated) writes **`global.ini`** to **both** queues with:

- `Output=C:\FileWisely\Incoming\<date>_<time>_<docname>.pdf`
- `ShowSaveAS=never`, `ShowSettings=never`, `ShowPDF=no`, `ConfirmOverwrite=no`

See [Hide the print dialog](https://www.bullzip.com/kb/hide-the-print-dialog/) (Bullzip KB).

**Already installed?** Run **`installer/repair-bullzip-silent.ps1`** as **Administrator** to re-apply the same settings.

## Windows “PDF” / “Document Created” toasts

Action Center notifications titled **PDF** with “Document Created …” come from the **Bullzip PDF Printer** (or Windows shell), **not** from the UCE overlay. UCE’s own upload toasts use the title **UCE — Universal Capture Engine**. To change or silence the driver toast, use **Windows Settings → System → Notifications** for that app, or check Bullzip’s docs for a `global.ini` key that disables success notifications (varies by version).

## After install

- Rename the printer to **FileWisely Printer** (must match UCE `print_config.rs`).
- Rely on **`install.ps1`** / **`repair-bullzip-silent.ps1`** for ProgramData **`global.ini`**. Legacy per-user **`%APPDATA%\Bullzip\PDF Printer\settings.ini`** is optional; see **`bullzip-settings.example.ini`**.

## Word / Office documents → PDF automatically

UCE watches **`C:\FileWisely\Incoming`** (and other folders in `uce-pdf-watch.json`). If a **`.doc` / `.docx`** lands there, UCE converts it to PDF with **LibreOffice** (headless) when `word_to_pdf_enabled` is true (default), then runs the normal upload pipeline.

**Install LibreOffice** on the workstation and ensure `soffice.exe` is on PATH or set `libreoffice_path` in **`uce-pdf-watch.json`**.

### Why “7 prints → 3 captured” can still happen (not a UCE pipeline bug)

Staging and claim **cannot** win every race if **Windows or CCC starts Microsoft Word** (or opens the `.docx` via association) **before** UCE’s watcher runs. Word locks the file → claim retries → some jobs never complete.

**Reliable fix:** do **not** send CCC print jobs through a Word-based workflow. **Ingestion format should be PDF from the printer:**

`CCC → FileWisely Printer (or PDF-only driver) → PDF in Incoming → UCE → upload`

Word as a **print target** or **“open after save”** step must be **removed from the shop workflow**, not tuned away in UCE.

**Quick check:** In CCC’s print dialog, the printer name should be **FileWisely Printer** (or another virtual **PDF** printer) — **not** Word, Office, or anything that launches WINWORD.EXE.

### If Microsoft Word opens or shows “Want to save your changes…”

UCE **never** launches Word; conversion uses **LibreOffice only** (`soffice --headless …`). If Word still appears:

1. **CCC print target** — Prefer **FileWisely Printer** / virtual PDF so output is **PDF** (or raw file save) instead of **Print to Microsoft Word** or a workflow that starts Word.
2. **Windows Explorer** — Turn off the **Preview pane** for `C:\FileWisely\Incoming` (preview can load Word handlers for `.docx`).
3. **UCE staging (always)** — UCE **claims** each new `.doc`/`.docx` by **moving** it into a hidden **`.uce_staging`** folder **before** any stability wait or LibreOffice step, so the visible Incoming path is cleared as fast as possible (reduces Explorer preview / shell handlers seeing a live file there). Stability and headless conversion run **only** on the staged copy.

If Word already has the file open from Incoming, close Word or the save dialog before conversion can finish.

### CCC / source workflow (avoid Word-based print)

- In CCC, use a **virtual PDF printer** (e.g. **FileWisely Printer**) so jobs land as **PDF** in `C:\FileWisely\Incoming`, not as “print to Word” or a DMS step that **opens Word** after print/save.
- Disable shop habits that **open the document after export** (any “open file after save” / preview in CCC or related tools).
- UCE logs **`[UCE] foreground … class=WINWORD|EXPLORER|…`** so you can confirm whether **WINWORD** or **explorer** is foreground during capture; adjust CCC and Explorer until **`class=OTHER`** or **`LIBREOFFICE`** during conversion when possible.

### Telemetry lines (console)

Look for:

- **`[UCE] claimed file`** — includes `detect_to_claim_ms` (fs watcher or poll start → successful rename into `.uce_staging`).
- **`[UCE] foreground …`** — active window exe, pid, short title, full path; **`class=`** is **`WINWORD`**, **`EXPLORER`**, **`LIBREOFFICE`**, or **`OTHER`**.
- **`[UCE] foreground parent:`** — parent PID/name of the **foreground** process (Windows only; uses PowerShell WMI once per successful claim).
- **`office_file_detected`**, **`office_claim_before_first_try`**, **`office_claim_retry`**, **`office_after_ingest_success`** — lifecycle markers.

### Rollout: reduce Explorer interference

- **Per-folder:** View → **Preview pane** → **Off** for `C:\FileWisely\Incoming`.
- **During validation tests:** Do **not** leave that folder open in Explorer; it triggers preview handlers.
- **Wider rollout:** Use your RMM/GPO to push the same Preview-pane-off expectation for shop machines (exact policy depends on Windows edition; document your org’s standard).

## Alternative drivers

- **PDF24** — often supports profiles / auto-save; same flow: output folder = Incoming, printer name = **FileWisely Printer**.
- **Microsoft Print to PDF** — always shows **Save As** unless replaced; fine for pilots, not for silent pipeline.
