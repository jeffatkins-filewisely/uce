# Tauri + Vanilla

This template should help get you started developing with Tauri in vanilla HTML, CSS and Javascript.

## GitHub releases and auto-update (UCE)

This folder is not automatically a Git repository: run `git init`, create a repo on GitHub under your org (for example `jeffatkins-filewisely/uce` or, after you move the repo, `filewisely/uce`. The URL in `tauri.conf.json` must match **owner/repo** exactly.), add the remote, and push.

1. **One-time signing key** — the public key is embedded in `src-tauri/tauri.conf.json` under `plugins.updater.pubkey` (must be **one line**, copied exactly from `.tauri/uce-signing.key.pub` — if this string is edited, the build fails with `Invalid padding`). The matching **private** key must stay secret: store it only in GitHub **Actions secrets** as `TAURI_SIGNING_PRIVATE_KEY` (and optional `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). To generate a new pair locally: `npx @tauri-apps/cli@2 signer generate -w .tauri/uce-signing.key --ci` (then paste the new **public** key from `.tauri/uce-signing.key.pub` into `tauri.conf.json`).
2. **Local release build** — `npm run tauri build` needs the private key in the environment when `createUpdaterArtifacts` is true. From the repo root: `powershell -ExecutionPolicy Bypass -File .\scripts\build-release.ps1` (or set `TAURI_SIGNING_PRIVATE_KEY` to the full text of `.tauri/uce-signing.key` yourself).
3. **Updater URL** — set `plugins.updater.endpoints` in `tauri.conf.json` to  
   `https://github.com/<org>/<repo>/releases/latest/download/latest.json` so it matches your real repo.
4. **Ship a release** — bump `package.json` / `src-tauri/tauri.conf.json` / `src-tauri/Cargo.toml` versions together, commit, then tag and push, for example:  
   `git tag v0.1.2 && git push origin v0.1.2`  
   The workflow in `.github/workflows/release.yml` builds the MSI, signs updater artifacts, and attaches them to the GitHub release. Installed clients poll for updates (see `maybeCheckForUceAppUpdate` in `src/main.js`).

### Business ID deep link (FileWisely web → desktop)

After install, users should not need to paste the UUID manually if the web app opens the desktop handler:

`uce://connect?business_id=<uuid>`

Example anchor/button: `href="uce://connect?business_id=${businessId}"` (or redirect to that URL from your edge function). UCE registers the `uce` scheme on Windows; a running instance receives the link via the single-instance + deep-link plugins.

## Ingestion pipeline (desktop → FileWisely backend)

**`docs/INGESTION_PIPELINE.md`** — end-to-end flow (watch → `read_pdf_file` → POST), **JSON field semantics**, and a **checklist for FileWisely/ingest** (provenance, `source_type` mapping, idempotency, classification). Point backend engineers there when wiring `uce_events` / capture inbox / Theo.

## CCC package sync (Sidekick → local CCC Import folders)

**`docs/CCC_PACKAGE_SYNC.md`** — claim batch → download → write → ack for crew photos headed to CCC ONE. Covers first-run **CCC Import** folder (`C:\FileWisely\CCC Import\` default), heartbeat `ccc_package_*` fields, 15s polling, tray status, and error/ack rules. Backend: `ccc-package-claim-batch` and `ccc-package-ack` edge functions.

## FileWisely PDF printer (Windows)

Prepare **Bullzip PDF Printer** for silent capture to `C:\FileWisely\Incoming\` (admin required). From the **repository root**:

```powershell
powershell -ExecutionPolicy Bypass -File .\installer\setup-filewisely-printer.ps1
```

Optional: add **`installer/pdf-printer/bullzip.exe`** so setup can install Bullzip silently. Full notes: **`installer/README-filewisely-printer.md`**.

## Troubleshooting: tiny “Hmm” tile or Edge error page in the overlay

WebView2 loads the UI from **`http://127.0.0.1:5173`** in development (see `tauri.conf.json` `devUrl`). If you start only the `.exe` while Vite is not running, the engine shows Chromium’s **“can’t reach this page”** (`chrome-error://…`) inside the small transparent window — it often looks like a clipped **“Hmm”** box.

**Fix:** from the repo root run `npm run tauri dev` (starts Vite with `--strictPort` on 5173, then UCE). For installed MSI builds, repair/reinstall so bundled assets under the app install path are intact.

UCE **resizes** to a readable size when that error is detected, **retries** reloading the webview a few times, then shows a Windows dialog if it still fails. Use the **system tray** menu **“Reload UCE Interface”** after starting Vite or repairing an install. In **debug** builds, startup checks port **5173** before showing the overlay so you are not left with only the tiny tile.

Navigation logging (`UCE_WEBVIEW_NAVIGATION_*`, `UCE_WEBVIEW_CURRENT_URL`) comes from Tauri’s global `on_page_load` hook. **`about:blank`** during the first startup check is treated as timing only; a second check runs a few seconds later before any recovery runs.

In **development**, recovery and tray reload call **`navigate()`** to `build.devUrl` (`http://127.0.0.1:5173`) instead of only **`reload()`**, because reloading **`about:blank`** never opens Vite (you would otherwise stay on blank through every retry). If the embedded config omits **`dev_url`**, the code falls back to **`http://127.0.0.1:5173/`** and logs **`UCE_WEBVIEW_RECOVERY_NAVIGATE_FALLBACK`**. After each navigate, recovery **polls** the WebView URL for several seconds (navigation is async; a single short sleep was incorrectly marking success before `127.0.0.1:5173` appeared in **`w.url()`**). Logs include **`UCE_WEBVIEW_RECOVERY_NAVIGATE`** and **`UCE_WEBVIEW_CURRENT_URL phase=recovery_poll_loaded`** when the UI commits.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
