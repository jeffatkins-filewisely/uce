# Red octagon / Live Mirror offline — UCE Sidekick (Tauri) fix brief

## Symptoms

- Windows tray shows a **red octagon** (missing tray PNG) instead of health dot
- Portal **Live Mirror** red with unclear cause until `ccc-tray-health` diagnosis

## Implemented in Sidekick (v0.1.56+)

### 1. `tauri-plugin-single-instance`

- Second launch focuses existing instance (`uce_tray_show_main`) and forwards `uce://` deeplinks

### 2. Embedded tray-icon fallback

- Tray built with **generated RGBA circle** (`device_health::embedded_tray_icon_initial`) — never `default_window_icon()` alone
- `refresh_tray` updates green / yellow / red from health snapshot

### 3. Rust watchdog + panic reporter

- `health_watchdog.rs`: stale heartbeat nudge, chrome-error recovery, `C:\FileWisely\logs\uce-health.log`
- `panic::set_hook` → `device_health.last_error` + log

### 4. Localhost probe

- `GET http://127.0.0.1:49217/whoami` → JSON `{ ok, product, agent_version, device_id, pid }`
- `local_agent_probe.rs`

### 5. `device_health.last_error` on heartbeat

- Populated from failed heartbeat, spool failures, panic, watchdog stale HB
- Sent on every ingest heartbeat for portal `ccc-tray-health`

## QA checklist

- [ ] Fresh install: tray shows **yellow/green dot**, not red octagon
- [ ] Double-click installer while running → existing window focuses, no second tray icon
- [ ] `curl http://127.0.0.1:49217/whoami` returns `ok: true` while UCE runs
- [ ] Disconnect network → heartbeat fails → portal shows `last_error` / diagnosis
- [ ] Pause sync from tray → portal `ccc_sync_paused` + diagnosis `paused`
- [ ] Tag build `v0.1.56` → GitHub Actions release MSI/NSIS

## Deploy

```bash
git tag v0.1.56
git push origin main
git push origin v0.1.56
```
