# UCE Awareness Layer — Desktop Agent (Tauri) Integration

This document is the contract between the **FileWisely `uce-ro-status` API** and the **UCE Tauri overlay** (`ccc-sidekick`). The API is authoritative for repair-order (RO) document completeness; the desktop agent is responsible for presentation, caching, and non-destructive guardrails.

### Production RO panel (not always visible)

The staff-facing panel (RO, Complete/Incomplete, Missing, Seen not captured, **Capture missing**, **Open FileWisely**) opens **only on demand**: click the **RO** toolbar button, or **right-click** the capture button. It does **not** stay open while you work — click **RO** again or **Esc** to close. Technical/debug text is limited to **Ctrl+right-click** on the capture button.

### Automatic `current_ro` (no user interaction)

The active window is polled every **500ms**. A window counts as a **CCC workflow** if any of these hold:

1. **`matched_rule` starts with `ccc_`** — includes built-in rules (`ccc_open`, `ccc_estimate`, …) and **Train**-learned rules (`ccc_trained_*`). The Rust matcher already treats titles with `repair order`, `ro `, `ro#`, etc. as `ccc_open` even when the literal string `CCC` does not appear (see `context_rules.rs`).
2. **Title contains `ccc` / `ccc one`** (legacy).
3. **Title looks like an RO line** — `RO` / `RO#` / `ro#` / `repair order`, or compact `ro` + 4–6 digits.

Then the RO number is taken from the title with **multiple patterns** (leading `90066`, `RO 90066`, `RO#90066`, `Repair Order 90066`, …), 4–6 digits. The value is stored as **`uce_current_ro`** plus a title snippet for the API. It **persists** when focus changes; it **only updates** when a **new** valid RO is detected. `uce-ro-status` uses this stored RO and title.

## Environment

| Variable | Required | Description |
|----------|----------|-------------|
| `VITE_UCE_RO_STATUS_URL` | Recommended | Full HTTPS URL of the RO status endpoint (e.g. Supabase Edge Function). If omitted, the app may derive a URL from `VITE_UCE_UPLOAD_URL` by replacing the last path segment `uce-ingest` → `uce-ro-status` (deployment-specific). |
| `VITE_UCE_UPLOAD_URL` | Yes (for ingest) | Used for upload; also used for URL derivation when `VITE_UCE_RO_STATUS_URL` is unset. |
| `VITE_SUPABASE_ANON_KEY` | Yes | Same Bearer + `apikey` headers as ingest. |

## HTTP contract

**Method:** `POST`  
**Headers:** `Content-Type: application/json`, `Authorization: Bearer <anon>`, `apikey: <anon>` (same as `uce-ingest`).

**Body:**

```json
{
  "business_id": "<uuid>",
  "repair_order_number": "<digits>",
  "ro_number": "<digits>",
  "window_title": "<optional — title from the window that showed the RO, often CCC; not the UCE overlay>",
  "device_id": "<uce stable device id>"
}
```

The client sends **both** `repair_order_number` and `ro_number` (same value). Some deployments (e.g. Supabase Edge Functions) validate only `ro_number`; others accept `repair_order_number` alone.

**Success (200):** JSON body (fields may be extended; unknown fields ignored by the client).

### Response shape (desktop-normalized)

| Field | Type | Description |
|-------|------|-------------|
| `completeness_level` | `"green"` \| `"yellow"` \| `"red"` | Drives capture button tint and passive alerts. |
| `missing_critical` | `string[]` | Human-readable labels; must be captured for “green” in strict workflows. |
| `missing_optional` | `string[]` | Nice-to-have. |
| `hints` | `string[]` | Short staff-facing messages (shown in panel + optional toasts). |
| `items` | `object[]` | Optional per-requirement rows (see below). |
| `in_system` | `object[]` | **In FileWisely** — documents already captured; see below (aliases: `documents_in_system`, `captured_documents`). |
| `recommended_captures` | `object[]` | Ordered list for “Capture missing docs” (criticality first). |

**`items[]` element (flexible):**

- `label` / `name` / `title` — display string  
- `status` — `"captured"` \| `"missing"` \| `"seen_not_captured"`  
  - **`missing`** — not yet in FileWisely (gap).  
  - **`seen_not_captured`** — backend signal that the doc exists in the shop workflow (e.g. CCC has a supplement/final bill), but **no capture is stored in FileWisely yet** (still need upload).  
  - **`captured`** — present in FileWisely for this RO.  
- `critical` — boolean  

Strings in `items` are treated as `{ label: string, status: "missing" }`.

**`recommended_captures[]` element (flexible):**

- `label` / `document_type` / `name` — shown in the action list  
- `critical` — boolean  
- `priority` — number (lower = sooner)  

**`in_system[]` element (flexible):**

- `label` / `name` / `title` / `document_type` — display string  
- `captured_at` / `uploaded_at` / `date` / `created_at` — optional; shown as a local date/time in the **✅ In System** panel section  

**Production RO panel (three sections):**

1. **✅ In System** — rows from `in_system` (count in header). If `in_system` is empty, the client may fall back to `items[]` where `status === "captured"` (including `captured_at` when present).  
2. **❌ Missing** — `missing_critical`, `missing_optional`, and `items[]` with `missing` (or equivalent).  
3. **👁 Seen, not captured** — **primarily UCE-local:** while the user works in CCC, the desktop agent records “seen” signals from **window title** heuristics (Estimate, Supplement #N, Final Bill, Print/Forms, Teardown, etc.) per RO in `localStorage`. Rows shown here are **not** already listed in **In System**. FileWisely may **append** `items[]` with `status: "seen_not_captured"` as a secondary hint; they are merged with the local list when not duplicate.

## Desktop behavior

### 1. RO identification

The agent extracts a repair order number from `window_title` (from `get_watch_context`) using heuristics such as:

- `RO#12345`, `RO 12345`, `RO-12345`  
- `Repair Order 12345`  

If no RO is found, RO status UI is hidden and the capture button uses default styling.

### 2. Caching and polling

| Mechanism | Interval / TTL | Purpose |
|-----------|----------------|---------|
| In-memory cache | **30s** TTL per `repair_order_number` | Avoid hammering the API on rapid context polls (~1.2s) and repeated right-clicks. |
| Background refresh | **60s** when an RO is present in context | Keeps badge and passive alerts reasonably fresh without tying to every context tick. |

Force-refresh bypasses cache (used on the 60s timer).

### 3. Capture button color mapping

Applied only when `VITE_UCE_RO_STATUS_URL` (or derived URL) resolves and a valid `repair_order_number` is present:

| `completeness_level` | Button class |
|---------------------|--------------|
| `green` | `uce-ro-level-green` |
| `yellow` | `uce-ro-level-yellow` |
| `red` | `uce-ro-level-red` |

Default blue gradient applies when RO status is unavailable or not applicable.

### 4. Right-click (pointer down button 2) — “status peek”

Layout (left to right when space allows; mirrored near left monitor edge):

1. **Debug / context block** — tenant, context line, window title, last upload debug (existing).  
2. **RO status widget** — completeness badge, checklist (Captured / Missing / Seen), critical/seen chips, hints, **Capture missing docs** summary (from `recommended_captures`).

### 5. Passive duplicate detection (PDF auto-upload)

Before uploading a PDF from the folder watcher, the agent checks an in-memory set of fingerprints:

`fingerprint = `${file_path}|${modified_unix_ms}``

If the same fingerprint was already uploaded successfully in this session, the upload is skipped (toast: duplicate skipped). Screenshots are not deduplicated this way.

### 6. Incomplete RO notifications

When `completeness_level` is `red`, or when `missing_critical` is non-empty (implementation may treat `yellow` with hints only as a softer signal):

- Show a **toast** at most once per **5 minutes** per `repair_order_number` (cooldown), to avoid spam while the estimator stays on the RO screen.

### 7. “Capture missing docs”

A control in the RO panel surfaces `recommended_captures` (prioritized). Activating it shows a **non-blocking toast** listing the next captures; it does not auto-open CCC. Staff follows shop workflow; the list is guidance only.

## Security

- No RO status request is sent without `business_id` and a non-empty `repair_order_number`.  
- Same anon key model as ingest; no new secrets on the desktop.

## Versioning

Bump `metadata.app_version` in ingest payloads independently; this spec references desktop behavior in `uce-universal-capture-engine-v1` lineage.
