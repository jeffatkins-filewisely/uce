# Phase B — Reliability (watchdog + upload spool)

Desktop agent **v0.1.55+** adds self-healing supervision and a durable outbound upload queue.

## Upload spool

When an auto-PDF upload throws (network, 5xx, timeout), the file is enqueued in:

`%APPDATA%\com.tauri.dev\uce-upload-spool.json` (Tauri app data dir)

- Survives app restarts
- Retries every **30s** (`uce:spool-drain`) and on startup (+ 60s interval)
- JS drains via existing `uploadCapture` (same metadata path as live uploads)
- Tray / heartbeat `device_health.spool_pending` shows queued count

IPC: `uce_spool_enqueue`, `uce_spool_claim_batch`, `uce_spool_ack`, `uce_spool_fail`, `uce_spool_pending_count`

## Health watchdog

Background loop every **60s** (`health_watchdog.rs`):

| Check | Action |
|--------|--------|
| Heartbeat stale > 10 min | Emit `uce:watchdog-heartbeat-nudge` → JS `sendUceHeartbeat()` |
| WebView on `chrome-error:` URL | Rate-limited silent reload (existing recovery) |
| CCC idle long | Append diagnostic only (poll loop continues) |

Log: `C:\FileWisely\logs\uce-health.log` (trimmed to ~400 lines)

## Deploy

Tag release to build installers:

```bash
git tag v0.1.55
git push origin v0.1.55
```
