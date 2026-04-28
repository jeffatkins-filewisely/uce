import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getUceDeviceId } from "./uceDeviceId.js";

const root = document.getElementById("root");

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
  <h1>Connection status</h1>
  <pre class="mono">${esc(JSON.stringify(j, null, 2))}</pre>
  <p class="mini">Last heartbeat (UTC): ${esc(lastHuman)}</p>
  <button type="button" id="cdRefresh">Refresh</button>
  <button type="button" id="cdCopy" class="secondary">Copy diagnostic report</button>
</div>`;

  document.getElementById("cdRefresh").addEventListener("click", () => {
    void renderStatus();
  });
  document.getElementById("cdCopy").addEventListener("click", async () => {
    try {
      await invoke("uce_copy_diagnostic_report");
      alert("Diagnostic report copied to clipboard (secrets masked).");
    } catch (e) {
      alert(String(e));
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
    pre.mono { white-space: pre-wrap; word-break: break-word; font-size: 0.78rem; background: #1a1a20; padding: 12px; border-radius: 8px; border: 1px solid #2a2a32; max-height: 360px; overflow: auto; }
    pre.out { margin-top: 14px; padding: 12px; border-radius: 8px; font-size: 0.82rem; white-space: pre-wrap; }
    pre.out.ok { background: #14532d; border: 1px solid #22c55e; }
    pre.out.bad { background: #450a0a; border: 1px solid #f87171; }
    .mini { font-size: 0.8rem; opacity: 0.75; margin-top: 10px; }
  `;
  document.head.appendChild(s);
}

async function main() {
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
