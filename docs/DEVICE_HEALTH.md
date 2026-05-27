# UCE device health (tray + heartbeat)

Phase A support visibility: colored tray icon, rich tooltip, and `device_health` on every ingest heartbeat.

## Tray colors

| Color | When |
|-------|------|
| `green` | Tenant configured, last heartbeat OK, not paused, CCC not offline |
| `yellow` | Sync paused, no heartbeat yet, or CCC package claim offline |
| `red` | Missing tenant credentials, heartbeat failed, or stale (>12 min since last OK) |

Tooltip (hover): multi-line status — shop, heartbeat, CCC sync, pending uploads, agent version.

## Heartbeat payload extension

```json
{
  "action": "heartbeat",
  "business_id": "...",
  "device_id": "...",
  "device_health": {
    "tray_state": "green",
    "tenant_configured": true,
    "business_id_short": "a1b2c3d4…",
    "heartbeat_ok": true,
    "heartbeat_stale": false,
    "last_heartbeat_unix_ms": 1710000000000,
    "last_heartbeat_category": "HEARTBEAT_OK",
    "ccc_sync_paused": false,
    "ccc_sync_offline": false,
    "ccc_syncing_count": 0,
    "last_ccc_sync_unix_ms": 1710000000000,
    "pending_uploads": 2,
    "spool_pending": 1,
    "last_upload_unix_ms": 1710000000000,
    "last_error": "",
    "agent_version": "0.1.56"
  }
}
```

**FileWisely backend:** persist `device_health` on the device row — implementation in **`backend/`** (migration + `uce-ingest` merge + Lovable table). See `backend/README.md`.

## IPC

| Command | Purpose |
|---------|---------|
| `uce_get_device_health_snapshot` | Full snapshot (JS merges into heartbeat) |
| `uce_report_upload_queue_stats` | JS reports `pendingUploads`, `lastUploadUnixMs` |
| `uce_refresh_tray_health` | Force tray icon + tooltip refresh |

Rust refreshes tray automatically on: heartbeat outcome, CCC sync poll, pause/resume, every 30s.

## Code

- `src-tauri/src/device_health.rs`
- `src-tauri/src/connection_diagnostics.rs` — `heartbeat_outcome()`
