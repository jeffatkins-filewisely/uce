# Tauri + Vanilla

This template should help get you started developing with Tauri in vanilla HTML, CSS and Javascript.

## GitHub releases and auto-update (UCE)

This folder is not automatically a Git repository: run `git init`, create a repo on GitHub under your org (for example `filewisely/uce` (GitHub org + repo name — no personal username in the URL)), add the remote, and push.

1. **One-time signing key** — the public key is embedded in `src-tauri/tauri.conf.json` under `plugins.updater.pubkey`. The matching **private** key must stay secret: store it only in GitHub **Actions secrets** as `TAURI_SIGNING_PRIVATE_KEY` (and optional `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). To generate a new pair locally: `npx @tauri-apps/cli@2 signer generate -w .tauri/uce-signing.key --ci` (then update `pubkey` in `tauri.conf.json` if you replace the key).
2. **Updater URL** — set `plugins.updater.endpoints` in `tauri.conf.json` to  
   `https://github.com/<org>/<repo>/releases/latest/download/latest.json` so it matches your real repo.
3. **Ship a release** — bump `package.json` / `src-tauri/tauri.conf.json` / `src-tauri/Cargo.toml` versions together, commit, then tag and push, for example:  
   `git tag v0.1.2 && git push origin v0.1.2`  
   The workflow in `.github/workflows/release.yml` builds the MSI, signs updater artifacts, and attaches them to the GitHub release. Installed clients poll for updates (see `maybeCheckForUceAppUpdate` in `src/main.js`).

## FileWisely PDF printer (Windows)

Prepare **Bullzip PDF Printer** for silent capture to `C:\FileWisely\Incoming\` (admin required). From the **repository root**:

```powershell
powershell -ExecutionPolicy Bypass -File .\installer\setup-filewisely-printer.ps1
```

Optional: add **`installer/pdf-printer/bullzip.exe`** so setup can install Bullzip silently. Full notes: **`installer/README-filewisely-printer.md`**.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
