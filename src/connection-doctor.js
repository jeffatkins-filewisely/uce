import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getUceDeviceId } from "./uceDeviceId.js";
import {
  getUceSuppressAllCached,
  initUcePopupSuppression,
  tracePopupSuppressed,
} from "./ucePopupSuppression.js";

const root = document.getElementById("root");

function guardedAlert(msg) {
  if (getUceSuppressAllCached()) {
    void tracePopupSuppressed("alert", "connection_doctor", msg);
    return;
  }
  alert(msg);
}

function params() {
  const q = new URLSearchParams(window.location.search);
  let view = q.get("view");
  if (!view && window.location.hash) {
    view = window.location.hash.replace(/^#/, "");
  }
  return { view: view === "status" ? "status" : "connect" };
}

function esc(s) {
  const d = document.createElement("div");
  d.textContent = s ?? "";
  return d.innerHTML;
}

async function renderConnect() {
  let cfg = {};
  try {
    cfg = await invoke("load_tenant_config");
  } catch (_) {
    cfg = {};
  }
  root.innerHTML = `
<div class="wrap">
  <h1>Connect to FileWisely</h1>
  <p class="hint">Paste values from FileWisely (Advanced Settings / Connect). Required for heartbeat and uploads.</p>
  <label>Business ID (UUID)</label>
  <input id="cdBiz" type="text" spellcheck="false" autocomplete="off" value="${esc(cfg.business_id || "")}" />
  <label>Backend / ingest URL (uce-ingest HTTPS endpoint)</label>
  <input id="cdUrl" type="text" spellcheck="false" autocomplete="off" value="${esc(cfg.backend_url || "")}" />
  <label>Supabase anon key (api key)</label>
  <input id="cdKey" type="password" spellcheck="false" autocomplete="off" value="${esc(cfg.anon_key || "")}" />
  <div class="row">
    <button type="button" id="cdSaveTest">Save &amp; Test Connection</button>
    <button type="button" id="cdSaveOnly" class="secondary">Save only</button>
  </div>
  <pre id="cdOut" class="out" hidden></pre>
</div>`;

  const uuidOk = (s) =>
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
      String(s || "").trim()
    );

  document.getElementById("cdSaveOnly").addEventListener("click", async () => {
    const biz = document.getElementById("cdBiz").value.trim();
    const url = document.getElementById("cdUrl").value.trim();
    const key = document.getElementById("cdKey").value.trim();
    if (!uuidOk(biz)) {
      showOut("Enter a valid business ID (UUID).", false);
      return;
    }
    try {
      await invoke("save_tenant_manual_all", {
        businessId: biz,
        backendUrl: url,
        anonKey: key,
      });
      await invoke("uce_refresh_tray_connection_tooltip");
      await emit("uce-tenant-saved", {});
      showOut("Saved to uce-tenant.json. Main UCE will reload tenant context.", true);
    } catch (e) {
      showOut(String(e), false);
    }
  });

  document.getElementById("cdSaveTest").addEventListener("click", async () => {
    const biz = document.getElementById("cdBiz").value.trim();
    const url = document.getElementById("cdUrl").value.trim();
    const key = document.getElementById("cdKey").value.trim();
    if (!uuidOk(biz)) {
      showOut("Enter a valid business ID (UUID).", false);
      return;
    }
    try {
      await invoke("save_tenant_manual_all", {
        businessId: biz,
        backendUrl: url,
        anonKey: key,
      });
      await invoke("uce_refresh_tray_connection_tooltip");
      await emit("uce-tenant-saved", {});
      const result = await invoke("uce_test_ingest_connection", {
        deviceId: getUceDeviceId(),
      });
      if (result.ok) {
        showOut(`Success — ${result.category} HTTP ${result.http_status ?? ""}`, true);
      } else {
        showOut(
          `Failed — ${result.category} HTTP ${result.http_status ?? ""}\n${result.message}`,
          false
        );
      }
    } catch (e) {
      showOut(String(e), false);
    }
  });

  function showOut(text, ok) {
    const el = document.getElementById("cdOut");
    el.hidden = false;
    el.textContent = text;
    el.className = "out " + (ok ? "ok" : "bad");
  }
}

async function renderStatus() {
  let j = {};
  try {
    j = await invoke("uce_get_connection_diagnostics");
  } catch (e) {
    j = { error: String(e) };
  }
  const ms = j.last_heartbeat_unix_ms;
  const lastHuman =
    typeof ms === "number" && ms > 0 ? new Date(ms).toISOString() : "never";

  root.innerHTML = `
<div class="wrap">
  <h1>Connection &amp; capture status</h1>
  <p class="hint">JSON below includes full <strong>capture_pipeline</strong>. <strong>Find CCC Folders</strong> scans typical CCC locations for recent PDFs/docs and saves paths to <code>auto_discovered_ccc_dirs</code> (watchers refresh ~60s). <strong>Copy diagnostic report</strong> includes capture health. <strong>CCC Capture Test</strong> waits for a <em>new</em> PDF (30s). If the test fails, auto-discovery runs once automatically. Global popup mute: <code>uce_suppress_all_popups</code> / env <code>UCE_SUPPRESS_ALL_POPUPS</code>. Printer-only: <code>uce_suppress_printer_severe_modal</code>.</p>
  <pre class="mono">${esc(JSON.stringify(j, null, 2))}</pre>
  <p class="mini">Last heartbeat (UTC): ${esc(lastHuman)}</p>
  <div class="row">
    <button type="button" id="cdRefresh">Refresh</button>
    <button type="button" id="cdCopy" class="secondary">Copy diagnostic report</button>
  </div>
  <div class="row">
    <button type="button" id="cdCccTest" class="secondary">Run CCC Capture Test (30s)</button>
    <button type="button" id="cdCccTestCopy" class="secondary" hidden>Copy CCC test result</button>
  </div>
  <pre id="cdCccTestOut" class="mono out" hidden></pre>
  <div class="row">
    <button type="button" id="cdCccDiscover" class="secondary">Find CCC Folders</button>
    <button type="button" id="cdCccDiscoverCopy" class="secondary" hidden>Copy discovery result</button>
  </div>
  <pre id="cdCccDiscoverOut" class="mono out" hidden></pre>
</div>`;

  document.getElementById("cdRefresh").addEventListener("click", () => {
    void renderStatus();
  });
  document.getElementById("cdCopy").addEventListener("click", async () => {
    try {
      await invoke("uce_copy_diagnostic_report");
      guardedAlert(
        "Diagnostic report copied (connection + capture pipeline health; secrets masked)."
      );
    } catch (e) {
      guardedAlert(String(e));
    }
  });

  const cccTestBtn = document.getElementById("cdCccTest");
  const cccTestOut = document.getElementById("cdCccTestOut");
  const cccTestCopy = document.getElementById("cdCccTestCopy");
  let lastCccTestResult = null;

  cccTestBtn.addEventListener("click", async () => {
    cccTestBtn.disabled = true;
    cccTestOut.hidden = false;
    cccTestOut.textContent =
      "Running… Waits up to 30s for a new PDF under CCC temp / C:\\CCC / ProgramData CCC. Export or print from CCC to that tree now.";
    cccTestOut.className = "mono out";
    cccTestCopy.hidden = true;
    lastCccTestResult = null;
    try {
      const result = await invoke("uce_ccc_capture_test", { timeoutSecs: 30 });
      lastCccTestResult = result;
      let text = JSON.stringify(result, null, 2);
      if (result && result.ok === false) {
        try {
          const disc = await invoke("uce_ccc_run_autodiscovery");
          text +=
            "\n\n--- Auto-discovery after failed CCC test ---\n" +
            JSON.stringify(disc, null, 2);
        } catch (e2) {
          text += `\n\n(auto-discovery error: ${e2})`;
        }
      }
      cccTestOut.textContent = text;
      cccTestOut.className = result?.ok ? "mono out ok" : "mono out bad";
      cccTestCopy.hidden = false;
    } catch (e) {
      cccTestOut.textContent = String(e);
      cccTestOut.className = "mono out bad";
    } finally {
      cccTestBtn.disabled = false;
    }
  });

  cccTestCopy.addEventListener("click", async () => {
    if (lastCccTestResult == null) return;
    try {
      await navigator.clipboard.writeText(
        JSON.stringify(lastCccTestResult, null, 2)
      );
      guardedAlert("CCC test result copied.");
    } catch (e) {
      guardedAlert(String(e));
    }
  });

  const discoverBtn = document.getElementById("cdCccDiscover");
  const discoverOut = document.getElementById("cdCccDiscoverOut");
  const discoverCopy = document.getElementById("cdCccDiscoverCopy");
  let lastDiscoverResult = null;

  discoverBtn.addEventListener("click", async () => {
    discoverBtn.disabled = true;
    discoverOut.hidden = false;
    discoverOut.textContent =
      "Scanning typical CCC paths for recent PDFs / CCC-like filenames…";
    discoverOut.className = "mono out";
    discoverCopy.hidden = true;
    lastDiscoverResult = null;
    try {
      const disc = await invoke("uce_ccc_run_autodiscovery");
      lastDiscoverResult = disc;
      discoverOut.textContent = JSON.stringify(disc, null, 2);
      discoverOut.className =
        disc?.ok === false ? "mono out bad" : "mono out ok";
      discoverCopy.hidden = false;
    } catch (e) {
      discoverOut.textContent = String(e);
      discoverOut.className = "mono out bad";
    } finally {
      discoverBtn.disabled = false;
    }
  });

  discoverCopy.addEventListener("click", async () => {
    if (lastDiscoverResult == null) return;
    try {
      await navigator.clipboard.writeText(
        JSON.stringify(lastDiscoverResult, null, 2)
      );
      guardedAlert("Discovery result copied.");
    } catch (e) {
      guardedAlert(String(e));
    }
  });
}

function injectStyles() {
  const s = document.createElement("style");
  s.textContent = `
    body { font-family: system-ui, Segoe UI, sans-serif; margin: 0; background: #141418; color: #e8e8ec; }
    .wrap { max-width: 520px; margin: 0 auto; padding: 20px 18px 28px; }
    h1 { font-size: 1.15rem; font-weight: 600; margin: 0 0 12px; }
    .hint { font-size: 0.85rem; opacity: 0.85; margin: 0 0 16px; line-height: 1.45; }
    label { display: block; font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.04em; margin: 12px 0 6px; opacity: 0.8; }
    input { width: 100%; box-sizing: border-box; padding: 10px 12px; border-radius: 8px; border: 1px solid #333; background: #1e1e24; color: inherit; font-size: 0.9rem; }
    .row { display: flex; gap: 10px; margin-top: 18px; flex-wrap: wrap; }
    button { padding: 10px 16px; border-radius: 8px; border: none; background: #3b82f6; color: #fff; font-weight: 600; cursor: pointer; font-size: 0.9rem; }
    button.secondary { background: #333; color: #ddd; }
    button:hover { filter: brightness(1.08); }
    pre.mono { white-space: pre-wrap; word-break: break-word; font-size: 0.78rem; background: #1a1a20; padding: 12px; border-radius: 8px; border: 1px solid #2a2a32; max-height: min(70vh, 520px); overflow: auto; }
    pre.out { margin-top: 14px; padding: 12px; border-radius: 8px; font-size: 0.82rem; white-space: pre-wrap; }
    pre.out.ok { background: #14532d; border: 1px solid #22c55e; }
    pre.out.bad { background: #450a0a; border: 1px solid #f87171; }
    .mini { font-size: 0.8rem; opacity: 0.75; margin-top: 10px; }
  `;
  document.head.appendChild(s);
}

async function main() {
  await initUcePopupSuppression(invoke);
  injectStyles();
  try {
    await getCurrentWindow().setTitle("UCE — FileWisely connection");
  } catch (_) {
    /* ignore */
  }
  const { view } = params();
  if (view === "status") await renderStatus();
  else await renderConnect();
}

void main();
