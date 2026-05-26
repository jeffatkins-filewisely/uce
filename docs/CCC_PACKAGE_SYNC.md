# CCC package sync (UCE Sidekick / Tauri)

FileWisely queues photos for CCC ONE import; **Sidekick** claims batches, downloads signed URLs, writes files under a local **CCC Import** tree, and acknowledges each item. No CCC ONE writes, folder watching, or Mitchell integration in this MVP.

Backend edge functions (FileWisely repo) must be deployed before sync works; this document is the **desktop contract**.

---

## 1. CCC Import folder (hardcoded)

| Item | Behavior |
|------|----------|
| **Path** | `C:\FileWisely\CCC Import\` on every shop PC (no folder picker) |
| **Storage** | `%AppData%\<app>\settings.json` records the same path + `first_run_completed` |
| **Tray** | **Open CCC Import Folder** — see `docs/TRAY_SILENT_INSTALL.md` |

Rust: `src-tauri/src/ccc_import_settings.rs` → `ensure_hardcoded_ccc_import_root()`

---

## 2. Heartbeat extension

Existing `uce-ingest` heartbeat (`action: "heartbeat"` in `src/main.js`) adds:

```json
{
  "ccc_package_capable": true,
  "ccc_package_root": "C:\\FileWisely\\CCC Import"
}
```

- `ccc_package_capable` — `true` when a non-empty root is configured.
- `ccc_package_root` — normalized absolute path (empty string when not capable).

---

## 3. Polling loop (every 15s when online)

**POST** `{SUPABASE_URL}/functions/v1/ccc-package-claim-batch`

- **Authorization:** `Bearer {anon_key}` from `uce-tenant.json` (same token as ingest / edge functions).
- **Body:** `{ "device_id": "<uce-device-id>", "limit": 25 }`
- **Device id:** Webview `localStorage` (`uceDeviceId.js`), synced to `uce-device-id.txt` via `uce_sync_device_id`.

**Response** — array of items (JSON field `items`), each with:

| Field | Example |
|-------|---------|
| `queue_id` | UUID |
| `ro_folder_name` | `RO81587_Smith_BMW` |
| `bucket` | `estimate` \| `check_in` \| `tear_down` \| `in_process` \| `check_out` |
| `signed_url` | HTTPS download URL (~5 min expiry) |
| `filename` | `photo.jpg` |
| `source_table` | pass through on ack |
| `source_id` | pass through on ack |

**Lease:** 2 minutes server-side; crash mid-batch re-queues unacked items.

Rust: `src-tauri/src/ccc_package_sync.rs` — `SUPABASE_URL` is derived from `backend_url` by truncating at `/functions/v1`.

---

## 4. Write to disk

For each item:

```
{ccc_package_root}\{ro_folder_name}\{bucket}\{filename}
```

- Creates parent folders as needed.
- Overwrites existing files (idempotent).

---

## 5. Ack

**POST** `{SUPABASE_URL}/functions/v1/ccc-package-ack`

```json
{
  "queue_id": "...",
  "source_table": "...",
  "source_id": "...",
  "status": "ok",
  "error_message": null
}
```

On `ok`, the server sets `ccc_package_ready_at` on the source photo (crew app badge → *Ready for CCC import*).

---

## 6. Tray menu

| Item | Action |
|------|--------|
| **CCC sync: …** | Disabled status line — `0 pending` / `N syncing…` / `Offline` |
| **Open CCC Import folder** | Opens root in Explorer |
| **Open this RO's folder** | Disabled until RO context exists (future) |
| **Change CCC Import folder…** | Folder picker + save |

---

## 7. Error handling

| Situation | Behavior |
|-----------|----------|
| Network failure on **claim** | No ack; tray **Offline**; retry next 15s poll |
| Disk full / permission denied | Ack `status: "error"` + message; server `ccc_package_error` |
| Signed URL expired (403/410) | Ack `error` with `error_message: "url_expired"`; server re-signs on next claim |
| Ack HTTP failure | Logged; item may re-queue after lease |

---

## Local files (support)

| File | Purpose |
|------|---------|
| `settings.json` | `ccc_package_root`, `first_run_completed` |
| `uce-device-id.txt` | Stable device id for claim batch |
| `uce-tenant.json` | `backend_url`, `anon_key`, `business_id` |

Log prefixes: `CCC_PACKAGE_CLAIM_*`, `CCC_PACKAGE_WRITTEN`, `CCC_PACKAGE_ACK_*`.

---

## Out of scope (MVP)

- Writing into CCC ONE
- Watching the CCC Import folder
- Mitchell / other DMS paths
