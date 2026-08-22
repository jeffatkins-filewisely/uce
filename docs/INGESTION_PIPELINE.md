# UCE ingestion pipeline & FileWisely backend checklist

This document describes what **UCE (this repo)** does through HTTP POST, and what **FileWisely / Supabase / edge ingest** should implement so captures become trustworthy inbox rows (RO linkage, `source_type`, Theo-ready documents).

---

## Part A — Desktop pipeline (implemented here)

### A.1 Watch → accept → emit (Rust)

| Step | Location | Notes |
|------|-----------|--------|
| Debounced file events | `src-tauri/src/services/print_watcher.rs` | One watcher thread per configured root. Scan-tagged roots also accept jpg/tif/png/bmp → PDF. |
| Print/scan folder learning | `src-tauri/src/services/source_autodiscovery.rs` | Seeds Windows scan dirs + learns new folders after WIA / print-to-PDF dialogs; persists `auto_discovered_source_dirs`. |
| Path handling | `handle_path` | Extension must be PDF or convertible Office |
| **Folder rule** | `pdf_watch_config::resolve_office_source_rule` | Longest-prefix match on watch roots (e.g. `general_downloads`, `ccc_temp`, `filewisely_incoming`) |
| General-folder filters | `pdf_watch_config.rs` | Ignores patterns (e.g. broad `%TEMP%` except `Temp\CCC`), min bytes for `general_*` |
| PDF stability | `wait_for_pdf_file_stable` | Avoids 0-byte / half-written files |
| Emit to UI | `incoming_emit::emit_uce_incoming_pdf_detailed` | Event `uce-incoming-file` + optional `modified_unix_ms`, `file_size`, **`capture_context`** (foreground + folder rule at emit); 2s later `uce-upload-pipeline-nudge` |
| Office files | `process_filewisely_office_incoming` → converter / printer | Produces PDF then same emit path |

### A.2 Read bytes → POST (JS + Rust IPC)

| Step | Location | Notes |
|------|-----------|--------|
| Listener | `src/main.js` | `listen("uce-incoming-file")` |
| Upload orchestration | `checkAutoPdfUpload`, `uploadFwPdfCore`, optional `uceForceUploadIncomingPdf` | Debounce, list merge, dedupe |
| Read file | `invoke("read_pdf_file", …)` → `main.rs` | Base64 PDF + canonical `file_path` + **`source_app` / `window_title`** (re-sampled at read) |
| IPC ACL | `src-tauri/permissions/uce-tenant-connection.toml` | `read_pdf_file` must stay allowed |
| HTTP | `uploadCapture` → `fetch(getBackendUploadUrl())` | Supabase-style anon JWT headers |

### A.3 CCC-only mode

If `UCE_CCC_TEMP_WATCH_ONLY` (or equivalent print config) is enabled, **`ccc_batch::handle_ccc_temp_file`** runs instead of the full general graph. Connection Doctor documents this; shops using full capture leave this off.

---

## Part B — Payload contract (what UCE sends today)

`uploadCapture` builds a JSON body (see `src/main.js`). Important fields:

| Field | Meaning |
|--------|---------|
| `business_id` | Tenant UUID |
| `file_base64` | PDF bytes (PDF mode) |
| `file_path` | Absolute path on disk (prefer canonical from `read_pdf_file`) |
| `source_path` | Same as `file_path` for PDFs when present — **explicit provenance for DB** |
| `source_hint` | Stable desktop hint, e.g. `uce_watcher_auto_pdf`, `uce_incoming_force_upload`, `uce_button_screenshot` (maps from JS `capture_source`) |
| `matched_rule` | From **foreground window context** (`get_last_observed_context`), **not** from Rust folder rule — often `"none"` when idle |
| `workflow_kind`, `bucket`, `action_allowed` | From same context snapshot |
| `classification`, `event_meta`, `metadata` | Mirror rules + device + `capture_source` |
| `document_type`, `desktop_document_subtype` | Derived in JS from context/title rules |

**Critical distinction for backend engineers**

- **Folder / “where did the file land?”** → use **`file_path`**, **`source_path`**, **`source_hint`**, and path-prefix rules on **`file_path`** (e.g. `Downloads`, `CCC`, `FileWisely\Incoming`).
- **Foreground CCC / app context** → **`matched_rule`** / **`window_title`** / **`workflow_kind`** from payload (may be weak if user switched windows).

---

## Part C — What to tell FileWisely to implement (backend / portal)

Implement or extend **uce-ingest** (or your capture Edge Function) as follows.

### C.1 Persist provenance on every successful ingest

- Store **`file_path`** and **`source_path`** on the event / document row (e.g. `uce_events.source_path`, `business_documents` equivalents).
- Map **`source_hint`** → internal **`source_type`** (string enum) and optional **`source_confidence`** (0–1), for example:

  | `source_hint` (examples) | Suggested `source_type` | Suggested confidence band |
  |--------------------------|---------------------------|----------------------------|
  | Paths under CCC / WORKFILES / Temp\CCC (derive from `file_path`) | `ccc_export` / `ccc_temp` | High |
  | `FileWisely\Incoming`, `fw_` naming | `filewisely_printer` | High |
  | `uce_watcher_auto_pdf` + Downloads/Desktop/Documents | `user_folder` | Medium |
  | `uce_incoming_force_upload` | `forced_diagnostic` | Use for debugging only |
  | Missing / unknown | `unknown` | Low |

  **Prefer deriving folder tier from `file_path`** when `source_hint` is generic, so Outlook/scanner folders added later “just work.”

### C.2 Do not rely on `matched_rule` alone for document class

- Treat **`matched_rule` from window context** as **supplementary** (CCC title detection when the user was focused on CCC).
- Merge with **path-based** `source_type` in a defined order (e.g. path wins for “where file lived,” window wins for “what app was focused” when both exist).

### C.3 Idempotency & duplicates

- Key uploads by **`business_id` + content hash or (`file_path` + `mtime`/`captured_at`)** to avoid duplicate inbox rows when UCE retries or debounce fires twice.

### C.4 Classification & RO linkage (server-side)

- Run your existing **document classification / RO extraction** on **`file_base64`** after persist.
- Attach **repair order** from filename/title/body rules; expose failures as **unmatched** with reason for the inbox UI.

### C.5 Theo / timeline (downstream)

- Theo should consume **normalized documents** that already include **`source_type`**, **`source_confidence`**, **`file_path`**, and RO linkage — **not** raw folder strings from the desktop.

### C.6 Observability

- Log ingest **`event_id`**, **`source_hint`**, **`file_path`**, and classification outcome for support.
- Surface HTTP **400/401/403** reasons to Connection Doctor–style tooling where possible.

---

## Part D — Optional future UCE enhancements (not required for backend)

- Default scan destinations (`Pictures\Scanned Documents`, Epson/Brother vendor folders, `C:\FileWisely\Scans`) are watched. Source autodiscovery (`source_autodiscovery.rs`) learns additional print/scan folders after WIA or print-to-PDF dialogs and persists them to `auto_discovered_source_dirs`. Scan images (jpg/tif/png/bmp) from those roots convert to PDF via LibreOffice. `\\.\Usbscan0` is a device port and cannot be watched as a folder.
- Optionally send Rust **`matched_rule`** (folder) as a separate field in the JSON body if product wants both without inference — **would require a small UCE change + backend field**.

---

## References in repo

- Watch roots & rules: `src-tauri/src/pdf_watch_config.rs`
- Watcher: `src-tauri/src/services/print_watcher.rs`
- Emit: `src-tauri/src/services/incoming_emit.rs`
- Upload: `src/main.js` → `uploadCapture`
- Tenant / ingest URL: `initTenantContext`, `getBackendUploadUrl`
