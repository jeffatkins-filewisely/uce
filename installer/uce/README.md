# UCE (Tauri) binaries

Copy your **release** build here before running `install.ps1`, for example:

- `tauri build` output under `src-tauri/target/release/` (or `bundle/` MSI/NSIS output).
- Include the main executable and any sidecars your build produces.

**Minimum:** one `*.exe` that launches the overlay (e.g. `ccc-sidekick.exe` or your renamed `uce.exe`).

`install.ps1` copies everything from this folder to `C:\FileWisely\App\` and creates a **Startup** shortcut to the first matching `*.exe` (name contains `uce`, `filewisely`, or `sidekick`), or the only `.exe` if one exists.
