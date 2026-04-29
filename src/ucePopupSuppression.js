/**
 * Global kill-switch for UCE toasts/alerts (native MessageBox uses env + Rust sync).
 * Production default: suppress ON. Opt out: localStorage uce_suppress_all_popups=0 or env UCE_SUPPRESS_ALL_POPUPS=0.
 */
import { invoke } from "@tauri-apps/api/core";

let suppressCache = import.meta.env.PROD === true;

export function getUceSuppressAllCached() {
  return suppressCache;
}

/** @param {string | undefined | null} envStr */
export function computeSuppressAllSync(envStr) {
  if (envStr === "0" || envStr === false) return false;
  if (envStr === "1" || envStr === true) return true;
  try {
    if (typeof localStorage !== "undefined") {
      if (localStorage.getItem("uce_suppress_all_popups") === "0") return false;
      if (localStorage.getItem("uce_suppress_all_popups") === "1") return true;
    }
  } catch (_) {
    /* ignore */
  }
  const v = (import.meta.env.VITE_UCE_SUPPRESS_ALL_POPUPS || "").trim();
  if (v === "0") return false;
  if (v === "1") return true;
  return import.meta.env.PROD === true;
}

/** Sync effective flag to Rust so MessageBox / native paths match JS. */
export async function initUcePopupSuppression(invokeFn) {
  let env = null;
  try {
    env = await invokeFn("uce_get_env_suppress_all_popups");
  } catch (_) {
    /* ignore */
  }
  suppressCache = computeSuppressAllSync(
    env === null || env === undefined ? undefined : String(env)
  );
  try {
    await invokeFn("uce_sync_suppress_all_popups", { suppress: suppressCache });
  } catch (_) {
    /* optional when command missing */
  }
  console.info(
    `[UCE] suppress_all_popups=${suppressCache} (UCE_SUPPRESS_ALL_POPUPS env, localStorage uce_suppress_all_popups, prod default ON)`
  );
}

export async function tracePopupSuppressed(kind, source, message) {
  const msg = message != null ? String(message) : "";
  const line = msg.replace(/\s+/g, " ").trim().slice(0, 400);
  console.info(`UCE_UI_POPUP_SUPPRESSED kind=${kind} source=${source} message=${line}`);
  try {
    await invoke("uce_ui_popup_suppressed_trace", {
      kind,
      source,
      message: msg || null,
    });
  } catch (_) {
    /* optional */
  }
}
