# FileWisely UCE Sidekick — tray + silent install (Tauri v2)

Shop-standard desktop agent: **no folder picker**, fixed **`C:\FileWisely\CCC Import\`**, system tray controls, per-user NSIS installer (no UAC).

This repo uses **Tauri 2** (not the v1.6 snippets in older specs). Behavior matches the product intent below.

---

## Install (zero UAC)

**`src-tauri/tauri.conf.json`**

- `bundle.targets`: `msi`, `nsis`
- `bundle.windows.nsis.installMode`: `currentUser` → `%LOCALAPPDATA%\Programs\FileWisely UCE\`
- `displayLanguageSelector`: `false`

Build:

```bash
npm run tauri build
```

Artifacts: `src-tauri/target/release/bundle/msi/` and `bundle/nsis/`. Prefer the **NSIS `.exe`** for silent per-user installs.

---

## Hardcoded CCC Import folder

| Path | Purpose |
|------|---------|
| `C:\FileWisely\CCC Import\` | Crew photos / CCC package sync (`{RO}\{bucket}\{file}`) |
| `C:\FileWisely\Incoming\` | Virtual printer PDF capture (see installer) |

On startup, Rust calls `ensure_hardcoded_ccc_import_root()` — **no Windows folder dialog**.

Webview: `ccc_import_hardcoded_root` IPC + `UCE_CCC_IMPORT_ROOT` in `main.js`. Heartbeat reports `ccc_package_capable: false` unless `UCE_CCC_LIVE_MIRROR=1` (FileWisely → CCC Import writer is off by default).

---

## System tray

Right-click (^ by clock):

| Item | Action |
|------|--------|
| **Open FileWisely UCE** | Show/focus overlay |
| **Open FileWisely Incoming** | Explorer → `C:\FileWisely\Incoming` (Live Mirror off). With `UCE_CCC_LIVE_MIRROR=1`, this item opens `C:\FileWisely\CCC Import` instead. |
| **Pause / Resume Mirror** | Disabled unless Live Mirror outbound is on. |
| **CCC: capture only** | Status line — CCC files upload to FileWisely; no FileWisely → CCC writer |
| *Support* | Connection Status, Connect, Copy Diagnostic Report, Reload |
| **Quit** | Exit process |

**Left-click** tray icon → show UCE (menu does not open on left-click).

### Tray health (green / yellow / red)

| Color | Meaning |
|-------|---------|
| Green | Connected, heartbeat OK |
| Yellow | Paused, CCC offline, or waiting for first heartbeat |
| Red | Not configured, heartbeat failed, or stale (>12 min) |

Hover tooltip: shop id, last heartbeat, CCC sync, pending uploads, version. See **`docs/DEVICE_HEALTH.md`**.

**Close (X)** on overlay → hides window; process keeps running (watchers, sync, heartbeat).

---

## Auto-start after reboot

`startup_shortcut.rs` on Windows setup:

- `%APPDATA%\...\Startup\FileWisely UCE.lnk`
- `HKCU\...\Run` → `FileWiselyUCE`

Also created when tenant is connected (`uce_ensure_startup_shortcut` from JS).

---

## Frontend events

```js
import { listen } from "@tauri-apps/api/event";

await listen("uce:pause", () => { /* heartbeat cleared in main.js */ });
await listen("uce:resume", () => { /* ensureUceDesktopPresence() */ });
```

---

## Acceptance checklist

- [ ] NSIS install: no UAC, per-user path under `%LOCALAPPDATA%`
- [ ] `C:\FileWisely\CCC Import\` exists after first launch
- [ ] Tray icon visible; right-click menu works
- [ ] Left-click shows UCE; close hides to tray
- [ ] Pause / Resume update status and stop CCC polling
- [ ] Reboot → UCE in tray (Startup + Run)
- [ ] No folder picker on first run

---

## Icons

Place under `src-tauri/icons/` (bundler): `icon.ico`, PNG sizes, optional `tray.png` (32×32). Tray uses app icon when `tray.png` is absent.
