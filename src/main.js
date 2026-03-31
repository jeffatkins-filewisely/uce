import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import {
  availableMonitors,
  currentMonitor,
  getCurrentWindow,
  LogicalSize,
  PhysicalPosition,
  primaryMonitor,
} from "@tauri-apps/api/window";

import { inferCccDocSignalsFromTitle } from "./uceCccTitleSignals.js";
import {
  buildRoSupplementTruthModel,
  findSequenceGaps,
  seenInCccPayloadFromSeenKey,
} from "./uceSupplementRoModel.js";
import { detectUceContext } from "./uceContextRecognition.js";
import {
  applyUceFloatingButtonChrome,
  getLastUceDetectedContext,
  preferredCaptureModeFromUceDetection,
} from "./uceFloatingButtonAction.js";
import {
  getLastUceOpportunity,
  getUceOperatorDebugSnapshot,
  stepUceOperatorState,
} from "./uceContextOperatorState.js";
import {
  getUceRecognitionSignals,
  recordFwPdfIncomingForContext,
  recordOfficePrintPromptForContext,
  recordUserActivity,
} from "./uceContextSignals.js";
import { buildUceReasonPayload } from "./uceContextReasonLayer.js";
import {
  armPendingMissingSuppressBuffer,
  getPendingCaptureSuppressMs,
  getSuppressAggressiveMissingUntilMs,
  getSystemConfidencePercentForRo,
  isPendingMissingSuppressActive,
  noteUceContextCaptureSuccess,
  noteUceContextDetectionEmitted,
} from "./uceTrustMetrics.js";

/** Last `decisionReasons` from operator reason layer (debug / Theo bridge). */
let lastUceDecisionReasons = [];

const BACKEND_UPLOAD_URL = (import.meta.env.VITE_UCE_UPLOAD_URL || "").trim();
/** FileWisely / Supabase anon key (Bearer + apikey). Supports VITE_SUPABASE_ANON_KEY or VITE_UCE_SUPABASE_ANON_KEY. */
const SUPABASE_ANON_KEY = (
  import.meta.env.VITE_UCE_SUPABASE_ANON_KEY ||
  import.meta.env.VITE_SUPABASE_ANON_KEY ||
  ""
).trim();
/** Resolved at startup: `uce-tenant.json` (per install) overrides `VITE_UCE_BUSINESS_ID` (build-time). */
let resolvedBusinessId = "";
/** HTTPS JSON endpoint (see watch_policy_sync.rs). When set, replaces local watch lists on a schedule. */
const WATCH_POLICY_URL = (import.meta.env.VITE_UCE_WATCH_POLICY_URL || "").trim();
const WATCH_POLICY_POLL_MS = Number(import.meta.env.VITE_UCE_WATCH_POLICY_POLL_MS) || 30 * 60 * 1000;
const UPLOAD_TIMEOUT_MS = 10000;
/** Active window poll for context + RO monitor (CCC title + leading digits). */
const CONTEXT_POLL_MS = 500;
const AUTO_PDF_POLL_MS = 3000;
/** Removed strict inter-upload cooldown so multi-print from CCC (batch) can all upload; duplicates still blocked by path|mtime fingerprint. */
/** RO status API: 30s response cache; 60s forced background refresh when an RO is in context. */
const RO_STATUS_CACHE_MS = 30_000;
const RO_STATUS_BACKGROUND_POLL_MS = 60_000;
/** Self-healing: printer check + optional repair (cooldown in JS). */
const SELF_HEAL_PRINTER_MS = 30_000;
const PRINTER_REPAIR_COOLDOWN_MS = 10 * 60 * 1000;
/** Backup call to the same auto-PDF upload path (primary poll stays faster). */
const INCOMING_UPLOAD_BACKUP_MS = 15_000;
/** Trailing debounce after last `uce-incoming-file` so CCC burst writes land before JS lists/uploads. */
const INCOMING_BATCH_DEBOUNCE_MS = 2500;
/** Scan Incoming for stranded `fw_*.pdf`; upload-only (no conversion). */
const FW_RESCUE_SCAN_MS = 3000;
/** Rescue only attempts uploads for files at least this old (mtime vs wall clock). */
const FW_UPLOAD_MIN_AGE_MS = 5000;
/** Portal parity debug line interval. */
const FW_PARITY_LOG_MS = 10_000;
/** After a failed upload, wait this long before rescue retries (reduces log spam). */
const FW_FAIL_RETRY_COOLDOWN_MS = 5000;

/** FileWisely staged PDFs: `C:\\FileWisely\\Incoming\\fw_*.pdf` (case-insensitive basename). */
function isFwPdfPath(p) {
  if (!p || typeof p !== "string") return false;
  const base = p.split(/[/\\]/).pop() || "";
  const lower = base.toLowerCase();
  return lower.startsWith("fw_") && lower.endsWith(".pdf");
}

/** Normalize to a stable Map key (Windows paths, lowercased). */
function fwKeyPath(p) {
  return String(p || "")
    .replace(/\//g, "\\")
    .toLowerCase();
}

/** Stderr + DevTools: FileWisely pipeline stages (matches Rust `uce_fw_pipeline_log`). */
async function logPipeline(stage, path, detailObj) {
  if (!path) return;
  let detail = null;
  if (detailObj !== undefined && detailObj !== null) {
    try {
      detail =
        typeof detailObj === "string" ? detailObj : JSON.stringify(detailObj);
    } catch (_) {
      detail = String(detailObj);
    }
  }
  try {
    await invoke("uce_fw_pipeline_log", { stage, path, detail });
  } catch (_) {
    /* optional when command missing */
  }
  if (detail) {
    console.info(`[UCE][pipeline] ${stage} path=${path}`, detailObj ?? detail);
  } else {
    console.info(`[UCE][pipeline] ${stage} path=${path}`);
  }
}

function createDefaultFwState() {
  return {
    /** Seen in scan or upload pipeline (explicit, not inferred from basename). */
    discoveredOnDisk: false,
    queuedForUpload: false,
    uploadStarted: false,
    uploadFinished: false,
    uploadFailed: false,
    lastFailAt: 0,
    inFlight: false,
    originalPath: null,
  };
}

/** Per-path lifecycle for fw_*.pdf — not inferred from fingerprints alone. */
const fwPdfStateByPath = new Map();
/** Paths signaled by `uce-incoming-file` (for upload source attribution). */
const fwPathsSeenFromIncoming = new Set();

let fwUploadSerialPromise = Promise.resolve();

function getFwState(rawPath) {
  const key = fwKeyPath(rawPath);
  if (!fwPdfStateByPath.has(key)) {
    fwPdfStateByPath.set(key, createDefaultFwState());
  }
  return fwPdfStateByPath.get(key);
}

function enqueueFwUpload(fn) {
  const run = fwUploadSerialPromise.then(() => fn());
  fwUploadSerialPromise = run.catch((e) => {
    console.error("[UCE] fw upload queue error:", e);
  });
  return run;
}

function queueFwUpload(rawPath, meta, source) {
  const st = getFwState(rawPath);
  st.discoveredOnDisk = true;
  st.queuedForUpload = true;
  return enqueueFwUpload(() => uploadFwPdfCore(rawPath, meta, source));
}

function inferFwUploadSource(rawPath, batchSource) {
  const key = fwKeyPath(rawPath);
  if (batchSource === "incoming-debounced" || batchSource === "incoming-queued") {
    return "watcher";
  }
  if (fwPathsSeenFromIncoming.has(key)) return "watcher";
  return "converted-office";
}

/**
 * Upload one fw_*.pdf by absolute path (event path + rescue). Uses `read_pdf_file`'s returned
 * path for upload payload and moves (handles rename from document.pdf → fw_*.pdf).
 */
async function uploadFwPdfCore(rawPath, meta, source) {
  const st = getFwState(rawPath);
  const isOfficeDerivedPdf = source === "converted-office";
  st.originalPath = st.originalPath || rawPath;
  st.discoveredOnDisk = true;
  const hadQueued = st.queuedForUpload;
  st.queuedForUpload = false;

  if (st.uploadFinished) {
    if (isOfficeDerivedPdf) {
      console.info(
        `[UCE] OFFICE_PIPELINE_RESULT path=${rawPath} result=skipped reason=already_uploaded`
      );
    }
    logFwUploadSummaryLine(rawPath, source, st, {
      hadQueued,
      didStart: false,
    });
    return { skipped: true, reason: "already_uploaded" };
  }

  const fp = `${rawPath}|${meta.modified_unix_ms}`;
  if (uploadedPdfFingerprints.has(fp)) {
    if (isOfficeDerivedPdf) {
      console.info(
        `[UCE] OFFICE_PIPELINE_RESULT path=${rawPath} result=skipped reason=fingerprint`
      );
    }
    st.uploadFinished = true;
    st.uploadFailed = false;
    logFwUploadSummaryLine(rawPath, source, st, {
      hadQueued,
      didStart: false,
    });
    return { skipped: true, reason: "fingerprint" };
  }

  let didStart = false;
  st.uploadStarted = true;
  st.inFlight = true;
  /** Authoritative path after `read_pdf_file` (must match on-disk fw_*.pdf). */
  let pipelinePath = rawPath;
  /** Set after a successful `uce_move_fw_pdf_outcome` so catch does not treat upload OK as failure. */
  let fwMoveOutcomeHandled = false;

  try {
    didStart = true;
    if (isOfficeDerivedPdf) {
      console.info(`[UCE] OFFICE_UPLOAD_STARTED path=${pipelinePath}`);
    }
    await logPipeline("PROCESSING_STARTED", pipelinePath, {
      source,
      modified_unix_ms: meta?.modified_unix_ms,
    });
    try {
      await invoke("uce_log_pdf_lifecycle", {
        phase: "upload_started",
        path: rawPath,
      });
    } catch (_) {
      /* optional command */
    }
    console.info(`[UCE] trace js_upload_started path=${rawPath}`);
    const capture = await invoke("read_pdf_file", { path: rawPath });
    pipelinePath = capture.file_path || rawPath;
    if (fwKeyPath(pipelinePath) !== fwKeyPath(rawPath)) {
      await logPipeline("PATH_MISMATCH", pipelinePath, {
        requested_path: rawPath,
        read_pdf_file_path: pipelinePath,
      });
    }
    const contextSnapshot = await getUploadContextSnapshot();
    const uploadResult = await uploadCapture(
      capture,
      contextSnapshot,
      "pdf",
      "auto_pdf_folder",
      pipelinePath
    );
    try {
      await invoke("uce_log_pdf_lifecycle", {
        phase: "upload_finished",
        path: pipelinePath,
        success: !uploadResult?.skipped,
      });
    } catch (_) {
      /* optional command */
    }
    lastAutoPdfKey = fp;
    lastAutoPdfUploadAt = Date.now();
    if (!uploadResult?.skipped) {
      uploadedPdfFingerprints.add(fp);
      st.uploadFinished = true;
      st.uploadFailed = false;
      emitUceContextCapturedIfRelevant({
        source: "auto_pdf_incoming",
        fileHint: pipelinePath.split(/[\\/]/).pop() || null,
      });
      await invoke("uce_move_fw_pdf_outcome", {
        sourcePath: pipelinePath,
        outcome: "processed",
        errorJson: null,
      });
      fwMoveOutcomeHandled = true;
    } else {
      st.uploadFinished = true;
      st.uploadFailed = false;
      await invoke("uce_move_fw_pdf_outcome", {
        sourcePath: pipelinePath,
        outcome: "failed",
        errorJson: JSON.stringify({
          reason: "upload_skipped",
          message: uploadResult?.message || "No endpoint / local only",
        }),
      });
      fwMoveOutcomeHandled = true;
    }
    const pdfName = pipelinePath.split(/[\\/]/).pop() || "recent.pdf";
    if (uploadResult?.skipped) {
      console.info(`[UCE] Upload skipped (no endpoint / local): ${pdfName}`);
    } else {
      console.info(`[UCE] Upload success: ${pdfName}`);
    }
    console.info(
      `[UCE] trace js_upload_finished path=${pipelinePath} skipped=${uploadResult?.skipped ? "true" : "false"}`
    );
    if (isOfficeDerivedPdf) {
      const skipped = !!uploadResult?.skipped;
      console.info(
        `[UCE] OFFICE_UPLOAD_FINISHED path=${pipelinePath} skipped=${skipped ? "true" : "false"}`
      );
      console.info(
        `[UCE] OFFICE_PIPELINE_RESULT path=${pipelinePath} result=${skipped ? "skipped" : "success"}`
      );
    }
    return {
      ok: true,
      uploadResult,
      pdfName,
      skipped: !!uploadResult?.skipped,
    };
  } catch (oneErr) {
    try {
      await invoke("uce_log_pdf_lifecycle", {
        phase: "upload_finished",
        path: pipelinePath,
        success: false,
      });
    } catch (_) {
      /* optional command */
    }
    if (!fwMoveOutcomeHandled) {
      await logPipeline("FINAL_ERROR", pipelinePath, {
        message:
          typeof oneErr === "string"
            ? oneErr
            : oneErr?.message || String(oneErr),
        stack: typeof oneErr === "object" && oneErr?.stack ? oneErr.stack : "",
      });
      try {
        await invoke("uce_move_fw_pdf_outcome", {
          sourcePath: pipelinePath,
          outcome: "failed",
          errorJson: JSON.stringify({
            stage: "uploadFwPdfCore",
            message:
              typeof oneErr === "string"
                ? oneErr
                : oneErr?.message || String(oneErr),
            stack:
              typeof oneErr === "object" && oneErr?.stack ? oneErr.stack : "",
          }),
        });
      } catch (moveErr) {
        await logPipeline("FINAL_ERROR", pipelinePath, {
          step: "uce_move_fw_pdf_outcome_after_error",
          message: String(moveErr),
        });
      }
    }
    st.uploadFailed = true;
    st.uploadFinished = false;
    st.lastFailAt = Date.now();
    if (isOfficeDerivedPdf) {
      console.info(
        `[UCE] OFFICE_UPLOAD_FINISHED path=${pipelinePath} failed=true`
      );
      console.info(
        `[UCE] OFFICE_PIPELINE_RESULT path=${pipelinePath} result=failed`
      );
    }
    console.info(`[UCE] trace js_upload_failed path=${pipelinePath}`);
    const pdfName = pipelinePath.split(/[\\/]/).pop() || "file.pdf";
    console.error(`[UCE] Upload failed: ${pdfName}`, oneErr);
    throw oneErr;
  } finally {
    st.inFlight = false;
    st.uploadStarted = false;
    st.queuedForUpload = false;
    logFwUploadSummaryLine(rawPath, source, st, { hadQueued, didStart });
  }
}

function logFwUploadSummaryLine(rawPath, source, st, { hadQueued, didStart }) {
  const yn = (v) => (v ? "yes" : "no");
  console.info(
    `[UCE][upload_summary] path=${rawPath} discovered=${yn(
      st.discoveredOnDisk
    )} queued=${yn(hadQueued)} started=${yn(didStart)} finished=${yn(
      st.uploadFinished
    )} failed=${st.uploadFailed ? "yes" : "no"} source=${source}`
  );
}

/** Rescue uploader: source of truth for stranded fw_*.pdf — does not use checkAutoPdfUpload or uce-incoming-file. */
async function fwRescueUploaderTick() {
  let rows = [];
  try {
    rows = await invoke("list_fw_pdf_metas_in_filewisely_incoming");
  } catch (e) {
    console.warn("[UCE][rescue] list failed:", e);
    return;
  }
  if (!Array.isArray(rows)) rows = [];
  const now = Date.now();
  for (const meta of rows) {
    const rawPath = meta?.file_path;
    if (!rawPath || !isFwPdfPath(rawPath)) continue;
    const st = getFwState(rawPath);
    st.discoveredOnDisk = true;
    console.info(`[UCE][rescue] seen path=${rawPath}`);
    const age = now - Number(meta.modified_unix_ms || 0);
    if (age < FW_UPLOAD_MIN_AGE_MS) {
      console.info(`[UCE][rescue] skip path=${rawPath} reason=too_new`);
      continue;
    }
    if (st.uploadFinished) {
      console.info(`[UCE][rescue] skip path=${rawPath} reason=already_uploaded`);
      continue;
    }
    if (st.inFlight) {
      console.info(
        `[UCE][rescue] skip path=${rawPath} reason=upload_in_progress`
      );
      continue;
    }
    if (st.queuedForUpload) {
      console.info(`[UCE][rescue] skip path=${rawPath} reason=already_queued`);
      continue;
    }
    if (
      st.uploadFailed &&
      st.lastFailAt &&
      now - st.lastFailAt < FW_FAIL_RETRY_COOLDOWN_MS
    ) {
      console.info(
        `[UCE][rescue] skip path=${rawPath} reason=upload_failed_recently`
      );
      continue;
    }
    console.info(`[UCE][rescue] enqueue path=${rawPath}`);
    st.queuedForUpload = true;
    void enqueueFwUpload(() => uploadFwPdfCore(rawPath, meta, "rescue"));
  }
}

async function logFwParitySummary() {
  let rows = [];
  try {
    rows = await invoke("list_fw_pdf_metas_in_filewisely_incoming");
  } catch (e) {
    console.warn("[UCE][parity] scan failed:", e);
    return;
  }
  if (!Array.isArray(rows)) rows = [];
  const local_fw_pdf_count = rows.filter(
    (r) => r?.file_path && isFwPdfPath(r.file_path)
  ).length;
  let queued = 0;
  let uploaded = 0;
  let pending = 0;
  let failed = 0;
  for (const r of rows) {
    const p = r?.file_path;
    if (!p || !isFwPdfPath(p)) continue;
    const st = getFwState(p);
    if (!st.discoveredOnDisk) st.discoveredOnDisk = true;
    if (st.uploadFinished) uploaded++;
    else if (st.uploadFailed) failed++;
    else if (st.queuedForUpload || st.inFlight) queued++;
    else pending++;
  }
  console.info(
    `[UCE][parity] local_fw_pdf_count=${local_fw_pdf_count} queued=${queued} uploaded=${uploaded} pending=${pending} failed=${failed}`
  );
}
/** Blocking banner + severe printer modal after sustained unhealthy state. */
const HEALTH_HARD_ALERT_MS = 120_000;
const HEALTH_BANNER_HEIGHT_LOGICAL = 34;
const UCE_EVENT_LOG_KEY = "uce_event_log_v1";
const UCE_EVENT_LOG_MAX = 500;

/** Set `localStorage.setItem("uce_ro_debug", "1")` to show raw `uce-ro-status` JSON in the RO panel. */
function isUceRoDebugRawEnabled() {
  try {
    return import.meta.env.DEV || localStorage.getItem("uce_ro_debug") === "1";
  } catch {
    return false;
  }
}

const appWindow = getCurrentWindow();
const appEl = document.querySelector("#app");

async function initTenantContext() {
  let fromFile = null;
  try {
    fromFile = await invoke("load_tenant_business_id");
  } catch (e) {
    console.error("[UCE] load tenant business_id:", e);
  }
  const fromEnv = (import.meta.env.VITE_UCE_BUSINESS_ID || "").trim();
  const fromDisk =
    typeof fromFile === "string" && fromFile.trim() ? fromFile.trim() : "";
  resolvedBusinessId = fromDisk || fromEnv;
}

function getBusinessId() {
  return resolvedBusinessId;
}

/**
 * Full URL for `uce-ro-status` (or deployment-specific name). Prefer explicit env; else derive from ingest URL.
 * See `docs/UCE_AWARENESS_LAYER_SPEC.md`.
 */
function getRoStatusUrl() {
  const explicit = (import.meta.env.VITE_UCE_RO_STATUS_URL || "").trim();
  if (explicit) return explicit;
  const upload = (BACKEND_UPLOAD_URL || "").trim();
  if (!upload) return "";
  try {
    const u = new URL(upload);
    const p = u.pathname.replace(/\/+$/, "");
    if (p.includes("uce-ingest")) {
      u.pathname = p.replace(/uce-ingest[^/]*$/, "uce-ro-status");
      return u.toString();
    }
  } catch (_) {
    /* ignore */
  }
  return "";
}

/**
 * Extract RO# from CCC-style window titles (leading digits, RO/RO#/ro#, repair order, etc.).
 * Order: leading digits first, then common CCC ONE / portal patterns.
 */
function extractRoFromTitleForMonitor(title) {
  if (!title || typeof title !== "string") return "";
  const t = title.trim().replace(/\u00a0/g, " ");
  const patterns = [
    /^(\d{4,6})\b/,
    /\bRO[#:\s-]*(\d{4,6})\b/i,
    /\bro[#:\s-]*(\d{4,6})\b/i,
    /\bRepair\s*Order[#:\s]*(\d{4,6})\b/i,
  ];
  for (const re of patterns) {
    const m = t.match(re);
    if (m && m[1]) return m[1];
  }
  return "";
}

/**
 * UCE is always-on-top; `get_active_window` often returns this app, not CCC — so titles lack RO#.
 * Detect our overlay so we can use the last non-UCE window for RO / FileWisely status.
 */
function isUceForegroundContext(ctx) {
  if (!ctx) return false;
  const title = (ctx.window_title || "").toLowerCase();
  const app = (ctx.source_app || "").toLowerCase();
  if (title.includes("universal capture engine")) return true;
  if (title.includes("uce —") || /^uce\s+[—-]/.test(title.trim())) return true;
  if (app === "uce" || app.endsWith("\\uce.exe") || app.endsWith("/uce.exe")) return true;
  return false;
}

/** Persisted current RO from automatic monitoring (CCC window + RO pattern in title). */
const CURRENT_RO_KEY = "uce_current_ro";
const CURRENT_RO_TITLE_KEY = "uce_current_ro_title";
const LEGACY_RO_KEY = "uce_last_ro_number";

/** Train (T) targets: CCC uses PDF/folder; others default to screenshot-only uploads. */
const UCE_TRAIN_WORKFLOWS = [
  "ccc",
  "tesla_epc",
  "parts_trader",
  "ops_trax",
  "generic",
];
const UCE_TRAIN_WORKFLOW_STORAGE_KEY = "uce_train_workflow";

function getTrainWorkflowTarget() {
  try {
    const v = localStorage.getItem(UCE_TRAIN_WORKFLOW_STORAGE_KEY);
    if (v && UCE_TRAIN_WORKFLOWS.includes(v)) return v;
  } catch (_) {
    /* ignore */
  }
  return "ccc";
}

function setTrainWorkflowTarget(wf) {
  try {
    if (wf && UCE_TRAIN_WORKFLOWS.includes(wf)) {
      localStorage.setItem(UCE_TRAIN_WORKFLOW_STORAGE_KEY, wf);
    }
  } catch (_) {
    /* ignore */
  }
}

function cycleTrainWorkflowTarget() {
  const cur = getTrainWorkflowTarget();
  const i = UCE_TRAIN_WORKFLOWS.indexOf(cur);
  const next = UCE_TRAIN_WORKFLOWS[(i + 1) % UCE_TRAIN_WORKFLOWS.length];
  setTrainWorkflowTarget(next);
  return next;
}

function workflowTrainLabel(wf) {
  const m = {
    ccc: "CCC (PDF + files)",
    tesla_epc: "Tesla EPC",
    parts_trader: "PartsTrader",
    ops_trax: "OPS Trax",
    generic: "Generic (screenshot)",
  };
  return m[wf] || wf;
}

/** Per-RO map of CCC UI “seen” signals (print panel, estimate, supplements, final bill) — local to this app. */
const CCC_SEEN_STORAGE_PREFIX = "uce_ccc_seen_";

function cccSeenStorageKey(ro) {
  return `${CCC_SEEN_STORAGE_PREFIX}${ro}`;
}

function loadCccSeenForRo(ro) {
  if (!ro || !/^\d{4,6}$/.test(String(ro))) return {};
  try {
    const raw = localStorage.getItem(cccSeenStorageKey(ro));
    if (!raw) return {};
    const o = JSON.parse(raw);
    return o && typeof o === "object" ? o : {};
  } catch (_) {
    return {};
  }
}

function saveCccSeenForRo(ro, map) {
  if (!ro || !/^\d{4,6}$/.test(String(ro))) return;
  try {
    localStorage.setItem(cccSeenStorageKey(ro), JSON.stringify(map));
  } catch (_) {
    /* ignore */
  }
}

function normalizeDocLabelForMatch(s) {
  return String(s || "")
    .toLowerCase()
    .replace(/\s+/g, " ")
    .replace(/[#]/g, "")
    .trim();
}

function isDocLabelInInSystemList(label, inSystem) {
  const n = normalizeDocLabelForMatch(label);
  if (!n) return false;
  for (const e of inSystem) {
    const lab = typeof e === "string" ? e : e.label;
    if (normalizeDocLabelForMatch(lab) === n) return true;
    if (n.includes(normalizeDocLabelForMatch(lab)) || normalizeDocLabelForMatch(lab).includes(n))
      return true;
  }
  return false;
}

function formatCapturedAtDisplay(v) {
  if (v == null || v === "") return "";
  if (typeof v === "number") {
    const d = new Date(v > 1e12 ? v : v * 1000);
    return Number.isNaN(d.getTime()) ? "" : d.toLocaleString();
  }
  const s = String(v).trim();
  const d = Date.parse(s);
  if (!Number.isNaN(d)) return new Date(d).toLocaleString();
  return s;
}

/**
 * While a CCC workflow window is foreground, record doc “seen” signals for the current RO (from window title).
 * When UCE is focused, use the last non-UCE snapshot (same as RO monitor) so Workfile Print titles still register.
 */
function recordCccSeenFromContext(ctx) {
  if (!ctx) return;
  const now = Date.now();
  let effectiveCtx = ctx;
  if (isUceForegroundContext(ctx)) {
    const snap =
      lastNonUceWatchContext &&
      now - lastNonUceWatchContextAt <= NON_UCE_CONTEXT_TTL_MS
        ? lastNonUceWatchContext
        : null;
    if (!snap) return;
    effectiveCtx = snap;
  }
  if (!isCccWorkflowWindow(effectiveCtx)) return;
  const ro = getMonitoredCurrentRo();
  if (!ro) return;
  const title = (effectiveCtx.window_title || "").trim();
  const signals = inferCccDocSignalsFromTitle(title);
  if (signals.length === 0) return;
  const existing = loadCccSeenForRo(ro);
  let changed = false;
  for (const s of signals) {
    if (!existing[s.key]) {
      existing[s.key] = { label: s.label, firstSeenAt: now };
      changed = true;
      const payload = seenInCccPayloadFromSeenKey(s.key, s.label, now);
      if (payload) {
        try {
          emit("uce-seen-in-ccc", {
            ro,
            related_type: payload.related_type,
            supplement_number: payload.supplement_number,
            timestamp: payload.timestamp.toISOString(),
            seen_in_ccc: true,
          });
        } catch (e) {
          console.warn("[UCE] uce-seen-in-ccc emit:", e);
        }
      }
    }
  }
  if (changed) saveCccSeenForRo(ro, existing);
}

/** In-memory copy of {@link loadPersistedCurrentRo}; updated when a new RO is seen on a CCC workflow window. */
let currentRoLive = "";

/**
 * Last `get_watch_context` snapshot where the foreground window was **not** this overlay.
 * UCE is always-on-top; when the user interacts with it, active window is UCE and titles
 * never contain CCC RO# — we reuse this snapshot for RO parsing until it goes stale.
 */
let lastNonUceWatchContext = null;
let lastNonUceWatchContextAt = 0;

/** How long we trust `lastNonUceWatchContext` while UCE stays focused (ms). */
const NON_UCE_CONTEXT_TTL_MS = 5 * 60_000;

/** Last “known good” context for PDF/capture heuristics — declared before RO monitor uses it in UCE fallback. */
let lastKnownContext = null;
let lastKnownContextAt = 0;

/**
 * Treat as CCC / RO workflow when Rust says PDF, workflow_kind is CCC, or legacy rule/title heuristics match.
 * Non-CCC workflows (Tesla EPC, PartsTrader, etc.) do not drive RO# from title alone.
 */
function isCccWorkflowWindow(ctx) {
  if (!ctx) return false;
  const pcm = (ctx.preferred_capture_mode || "").toLowerCase();
  if (pcm === "pdf") return true;
  const wk = (ctx.workflow_kind || "").toLowerCase();
  if (wk === "ccc") return true;
  const rule = (ctx.matched_rule || "").toLowerCase();
  if (rule.startsWith("ccc_") || rule.startsWith("ccc_trained")) return true;
  const title = (ctx.window_title || "").toLowerCase();
  // CCC ONE often uses titles that start with RO# only: "90066 - Customer - Vehicle…" (no "CCC" / "RO" text).
  if (/^\d{4,6}\b/.test(title.trim())) return true;
  if (title.includes("ccc") || title.includes("ccc one")) return true;
  if (
    /\b(ro[#\s-]|ro\s|repair\s*order)/i.test(title) ||
    /\bro\d{4,6}\b/i.test(title)
  ) {
    return true;
  }
  // Workfile Print and similar modals: title often has no RO#, "CCC", or "RO" text.
  if (/\bworkfile\b/.test(title) && /\bprint\b/.test(title)) {
    return true;
  }
  return false;
}

/**
 * On each poll: if this is a CCC workflow window, extract RO# from the title → `current_ro`.
 * When UCE is foreground, use the last non-UCE snapshot (same tick the user is usually on CCC).
 * Persists across focus changes; updates only when a different valid RO is detected.
 */
function applyActiveWindowRoMonitor(ctx) {
  if (!ctx) return;
  const now = Date.now();

  if (!isUceForegroundContext(ctx)) {
    lastNonUceWatchContext = ctx;
    lastNonUceWatchContextAt = now;
    if (!isCccWorkflowWindow(ctx)) return;
    const title = (ctx.window_title || "").trim();
    const ro = extractRoFromTitleForMonitor(title);
    if (!ro) return;
    if (ro === getMonitoredCurrentRo()) return;
    saveCurrentRo(ro, title);
    return;
  }

  const snap =
    lastNonUceWatchContext &&
    now - lastNonUceWatchContextAt <= NON_UCE_CONTEXT_TTL_MS
      ? lastNonUceWatchContext
      : null;
  if (!snap) return;
  if (!isCccWorkflowWindow(snap)) return;
  const title = (snap.window_title || "").trim();
  const ro = extractRoFromTitleForMonitor(title);
  if (!ro) return;
  if (ro === getMonitoredCurrentRo()) return;
  saveCurrentRo(ro, title);
}

function saveCurrentRo(ro, title) {
  if (!ro || typeof ro !== "string" || !/^\d{4,6}$/.test(ro)) return;
  currentRoLive = ro;
  try {
    localStorage.setItem(CURRENT_RO_KEY, ro);
    if (title) {
      localStorage.setItem(CURRENT_RO_TITLE_KEY, title.slice(0, 500));
    }
  } catch (_) {
    /* ignore */
  }
  updateRoToolbarLabel();
}

function loadPersistedCurrentRo() {
  try {
    let v = localStorage.getItem(CURRENT_RO_KEY);
    if (!v) v = localStorage.getItem(LEGACY_RO_KEY);
    return v && /^\d{4,6}$/.test(v) ? v : "";
  } catch (_) {
    return "";
  }
}

function loadPersistedCurrentRoTitle() {
  try {
    return localStorage.getItem(CURRENT_RO_TITLE_KEY) || "";
  } catch (_) {
    return "";
  }
}

function getMonitoredCurrentRo() {
  return currentRoLive || loadPersistedCurrentRo();
}

/** Positive loop for Theo: FileWisely received a capture while UCE had a stable known workflow context. */
function emitUceContextCapturedIfRelevant({ source, fileHint }) {
  const stable = getLastUceDetectedContext();
  const interesting = new Set([
    "ccc_supplement",
    "ccc_estimate",
    "ccc_final_bill",
    "ccc_print_dialog",
    "tesla_epc",
    "parts_invoice",
  ]);
  if (!stable || stable.bucket !== "known" || !interesting.has(stable.type)) {
    return;
  }
  const ro = getMonitoredCurrentRo() || null;
  if (ro) noteUceContextCaptureSuccess(ro);
  emit("uce-context-captured", {
    type: stable.type,
    roId: ro,
    timestamp: new Date().toISOString(),
    source: source || "unknown",
    fileHint: fileHint || null,
    preferredCaptureMode: stable.preferredCaptureMode,
    decisionReasons: lastUceDecisionReasons.length
      ? [...lastUceDecisionReasons]
      : [],
  }).catch((e) => console.warn("[UCE] uce-context-captured emit failed:", e));
}

function getMonitoredWindowTitleForApi() {
  return loadPersistedCurrentRoTitle();
}

/**
 * RO# and title for `uce-ro-status` — from automatic monitoring + persistence only.
 * @returns {{ ro: string, windowTitleForApi: string, sourceLabel: string }}
 */
function resolveRoForAwareness() {
  const ro = getMonitoredCurrentRo();
  return {
    ro,
    windowTitleForApi: getMonitoredWindowTitleForApi(),
    sourceLabel: "",
  };
}

function updateRoToolbarLabel() {
  if (!uceRoPanelBtn) return;
  const r = getMonitoredCurrentRo();
  const next = r || "RO";
  const prev = uceRoPanelBtn.textContent;
  uceRoPanelBtn.textContent = next;
  uceRoPanelBtn.classList.toggle("uce-ro-has-ro", !!r);
  uceRoPanelBtn.title = r
    ? `RO ${r} — document status (FileWisely)`
    : "Repair order status";
  if (prev !== next) void setCompactWindowSize();
}

appEl.innerHTML = `
<style>
html, body {
  margin: 0;
  padding: 0;
  width: auto;
  height: auto;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: transparent !important;
  pointer-events: none;
}

/* Shrink-wrap the WebView’s interactive surface; avoid a full-window transparent hit slab (WebView2). */
#app {
  margin: 0;
  padding: 0;
  position: relative;
  display: inline-block;
  width: fit-content;
  height: fit-content;
  min-width: 0;
  min-height: 0;
  max-width: 100vw;
  overflow: visible;
  vertical-align: top;
  user-select: none;
  box-sizing: border-box;
  background: transparent !important;
  pointer-events: none;
}

/* Right-click / RO panel: column — toolbar on top, sheet drops below (top-left window anchor in JS). */
#app.uce-debug-open {
  display: flex;
  flex-direction: column;
  align-items: stretch;
  justify-content: flex-start;
  gap: 8px;
  flex-wrap: nowrap;
  padding: 8px 10px 10px;
  box-sizing: border-box;
  width: 100%;
  height: fit-content;
  max-width: none;
}

.uce-debug-sheet {
  display: none;
  flex: 0 0 auto;
  flex-direction: column;
  gap: 10px;
  width: 300px;
  max-width: min(340px, 92vw);
  max-height: 360px;
  overflow: auto;
  padding: 10px 12px;
  border-radius: 8px;
  font-family: "Segoe UI", sans-serif;
  font-size: 12px;
  line-height: 1.45;
  color: #f1f5f9;
  background: rgba(17, 24, 39, 0.97);
  white-space: pre-line;
  word-wrap: break-word;
  overflow-wrap: anywhere;
  box-sizing: border-box;
  -webkit-font-smoothing: antialiased;
}

.uce-debug-pre {
  margin: 0;
  white-space: pre-wrap;
  font: inherit;
  color: inherit;
}

#app.uce-debug-open .uce-debug-sheet {
  display: flex;
  position: relative;
  order: 2;
  z-index: 12;
  width: 100%;
  max-width: none;
}

/* Production RO / FileWisely panel — green theme (matches RO tile) */
.uce-debug-sheet.uce-debug-sheet--prod {
  background: linear-gradient(
    165deg,
    rgba(6, 95, 70, 0.94) 0%,
    rgba(15, 23, 42, 0.97) 52%,
    rgba(17, 24, 39, 0.99) 100%
  );
  border: 1px solid rgba(34, 197, 94, 0.5);
  box-shadow: 0 8px 28px rgba(22, 101, 52, 0.35);
  max-width: min(300px, 92vw);
  padding: 8px 10px;
  gap: 6px;
  max-height: min(380px, calc(100vh - 48px));
  overflow-y: auto;
  scrollbar-width: thin;
  scrollbar-color: rgba(34, 197, 94, 0.45) rgba(15, 23, 42, 0.5);
}

.uce-debug-sheet.uce-debug-sheet--prod::-webkit-scrollbar {
  width: 8px;
}

.uce-debug-sheet.uce-debug-sheet--prod::-webkit-scrollbar-thumb {
  background: rgba(34, 197, 94, 0.45);
  border-radius: 4px;
}

.uce-debug-sheet.uce-debug-sheet--prod::-webkit-scrollbar-track {
  background: rgba(15, 23, 42, 0.45);
  border-radius: 4px;
}

.uce-debug-sheet.uce-debug-sheet--prod .uce-prod-head {
  color: #ecfdf5;
}

.uce-debug-sheet.uce-debug-sheet--prod .uce-prod-k {
  color: #6ee7b7;
}

.uce-debug-sheet.uce-debug-sheet--prod .uce-prod-v {
  color: #f0fdf4;
}

.uce-debug-sheet.uce-debug-sheet--prod .uce-prod-v.uce-prod-muted {
  color: #a7f3d0;
}

.uce-debug-sheet.uce-debug-sheet--prod .uce-prod-list {
  color: #d1fae5;
}

.uce-debug-sheet.uce-debug-sheet--prod .uce-prod-empty {
  color: rgba(167, 243, 208, 0.65);
}

.uce-debug-sheet.uce-debug-sheet--prod .uce-prod-msg {
  color: #a7f3d0;
}

.uce-debug-sheet.uce-debug-sheet--prod .uce-prod-hint {
  color: rgba(110, 231, 183, 0.75);
  border-top-color: rgba(34, 197, 94, 0.28);
}

.uce-debug-sheet.uce-debug-sheet--prod .uce-prod-subhint {
  color: rgba(167, 243, 208, 0.72);
}

.uce-debug-sheet.uce-debug-sheet--prod .uce-prod-btn-primary {
  background: linear-gradient(180deg, #22c55e 0%, #15803d 100%);
}

.uce-debug-sheet.uce-debug-sheet--prod .uce-prod-btn-secondary {
  background: rgba(6, 78, 59, 0.65);
  border: 1px solid rgba(34, 197, 94, 0.35);
  color: #ecfdf5;
}

.uce-debug-sheet.uce-debug-sheet--prod .uce-prod-btn-secondary:hover {
  background: rgba(6, 95, 70, 0.85);
}

#app.uce-debug-open .uce-blocking-banner {
  order: 0;
  margin: 0 0 4px;
  width: 100%;
  max-width: none;
}

/* Toast stays fixed; not a flex item when debug row is open */
#app.uce-debug-open .toast {
  position: fixed;
}

/* In-flow when compact so #app shrink-wraps; toast/debug modes override below. */
#uceDock {
  position: relative;
  left: auto;
  top: auto;
  z-index: 6;
  display: inline-flex;
  flex-direction: column;
  align-items: stretch;
  width: max-content;
  height: max-content;
  max-width: 100%;
  pointer-events: none;
  opacity: 0.94;
  transition: opacity 0.2s ease;
  box-sizing: border-box;
  gap: 2px;
}

#uceDock.uce-dock--hot,
#uceDock.uce-dock--opaque {
  opacity: 1;
}

@keyframes uce-dock-snap-settle {
  0%,
  100% {
    transform: scale(1);
  }
  55% {
    transform: scale(1.045);
  }
}

#uceDock.uce-dock--snap-bump .uce-toolbar {
  animation: uce-dock-snap-settle 240ms ease;
}

#app.uce-debug-open #uceDock {
  order: 1;
  position: static;
  transform: none;
  flex-shrink: 0;
  align-self: stretch;
  z-index: 1;
}

#app.uce-debug-open #uceDock .uce-toolbar {
  position: static;
  transform: none;
  flex-shrink: 0;
  align-self: stretch;
  z-index: 1;
}

/* In-flow with dock so native window can match measured toolbar bounds. */
/* Opaque bar so WebView2 transparency does not “eat” the capture button (looked like an empty tile). */
.uce-toolbar {
  position: relative;
  left: auto;
  top: auto;
  transform: none;
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 4px;
  margin: 0;
  padding: 3px 6px;
  pointer-events: none;
  box-sizing: border-box;
  background: rgba(15, 23, 42, 0.94);
  border-radius: 12px;
  border: 1px solid rgba(148, 163, 184, 0.45);
  box-shadow: 0 4px 18px rgba(0, 0, 0, 0.45);
}

.uce-toolbar > button,
.uce-toolbar > .uce-health {
  pointer-events: auto;
}

.uce-debug-sheet,
.uce-blocking-banner,
.uce-tenant-setup,
.uce-printer-severe-modal {
  pointer-events: auto;
}

/* Toast: expand #app with the enlarged native window (fitWindowToToast). */
#app.uce-toast-open:not(.uce-debug-open) {
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  min-height: 100%;
  align-items: stretch;
}
#app.uce-toast-open:not(.uce-debug-open) #uceDock {
  position: static;
  margin-top: auto;
  margin-bottom: 4px;
  align-self: stretch;
  display: block;
}
#app.uce-toast-open:not(.uce-debug-open) #uceDock .uce-toolbar {
  position: static;
  top: auto;
  left: auto;
  transform: none;
  align-self: stretch;
  justify-content: flex-start;
  padding: 0 4px;
}

/* Wrapper is click-through; only the circle receives hits (no oversized drag slab). */
.uce-capture-wrap {
  position: relative;
  width: 38px;
  height: 38px;
  flex-shrink: 0;
  pointer-events: none;
}

.uce-capture-wrap .uce-btn {
  position: absolute;
  left: 0;
  top: 0;
  width: 38px;
  height: 38px;
  pointer-events: auto;
}

.uce-btn {
  border: 2px solid rgba(255, 255, 255, 0.35);
  border-radius: 9999px;
  /* Default chrome only — RO/context level classes override background. */
  background: linear-gradient(180deg, #4d7ec4 0%, #2f5599 100%);
  color: #fff;
  display: grid;
  place-items: center;
  cursor: grab;
  opacity: 1;
  transition: transform 120ms ease, filter 150ms ease, box-shadow 180ms ease;
  box-shadow: 0 0 0 1px rgba(15, 23, 42, 0.4), 0 2px 8px rgba(0, 0, 0, 0.35);
}

.uce-btn:hover {
  filter: brightness(1.06);
  transform: scale(1.02);
}

.uce-btn:active {
  cursor: grabbing;
  transform: scale(0.96);
  filter: brightness(0.94);
}

.uce-train {
  flex-shrink: 0;
  width: 22px;
  height: 32px;
  margin: 0;
  padding: 0;
  border: none;
  border-radius: 6px;
  font-size: 10px;
  font-weight: 700;
  font-family: "Segoe UI", sans-serif;
  color: #fff;
  background: linear-gradient(180deg, #f59e0b 0%, #d97706 100%);
  cursor: pointer;
  opacity: 0.82;
  line-height: 1;
  transition: filter 120ms ease, opacity 120ms ease;
}

.uce-train:hover {
  filter: brightness(1.06);
  opacity: 0.95;
}

.uce-train:active {
  filter: brightness(0.94);
}

.uce-train:disabled {
  opacity: 0.4;
  pointer-events: none;
}

.uce-train.uce-hidden {
  display: none !important;
}

.uce-settings {
  flex-shrink: 0;
  width: 22px;
  height: 32px;
  margin: 0;
  padding: 0;
  border: none;
  border-radius: 6px;
  font-size: 11px;
  line-height: 1;
  color: rgba(255, 255, 255, 0.75);
  background: rgba(30, 41, 59, 0.55);
  cursor: pointer;
  display: grid;
  place-items: center;
  transition: background 120ms ease, color 120ms ease;
}

.uce-settings:hover {
  color: #fff;
  background: rgba(51, 65, 85, 0.75);
}

.uce-settings:active {
  background: rgba(30, 41, 59, 0.9);
}

.uce-ro-toggle {
  flex-shrink: 0;
  min-width: 26px;
  height: 32px;
  max-height: 32px;
  margin: 0;
  padding: 0 6px;
  border: none;
  border-radius: 6px;
  font-size: 9px;
  font-weight: 800;
  font-family: "Segoe UI", sans-serif;
  letter-spacing: 0.02em;
  color: #e2e8f0;
  background: rgba(30, 41, 59, 0.75);
  cursor: pointer;
  line-height: 1;
  transition: background 120ms ease, color 120ms ease;
  box-sizing: border-box;
}

.uce-ro-toggle:hover {
  color: #fff;
  background: rgba(51, 65, 85, 0.9);
}

/* RO# detected from title — green tile (distinct from capture button completeness colors). */
.uce-ro-toggle.uce-ro-has-ro {
  color: #fff;
  background: linear-gradient(180deg, #22c55e 0%, #15803d 100%);
}

.uce-ro-toggle.uce-ro-has-ro:hover {
  color: #fff;
  background: linear-gradient(180deg, #34d399 0%, #16a34a 100%);
}

.uce-btn.busy {
  pointer-events: none;
  opacity: 0.8;
}

.uce-btn.quiet {
  opacity: 0.62;
  filter: saturate(0.85);
}

.uce-btn.active {
  opacity: 1;
  filter: saturate(1.08);
  animation: uce-breathe 1.8s ease-in-out infinite;
}

/* Avoid pulsing the capture button while the amber toast is open — combined with window resize it looked like violent flashing. */
#app.uce-toast-open .uce-btn.active {
  animation: none;
}

.uce-btn.uce-ro-level-green {
  background: linear-gradient(180deg, #22c55e 0%, #15803d 100%);
}

.uce-btn.uce-ro-level-yellow {
  background: linear-gradient(180deg, #eab308 0%, #ca8a04 100%);
}

.uce-btn.uce-ro-level-red {
  background: linear-gradient(180deg, #ef4444 0%, #b91c1c 100%);
}

.uce-btn.uce-btn--context-glow {
  box-shadow: 0 0 0 1px rgba(96, 165, 250, 0.38), 0 0 14px rgba(59, 130, 246, 0.3);
}

.uce-btn.uce-ctx-glow--estimate {
  box-shadow: 0 0 0 1px rgba(59, 130, 246, 0.48), 0 0 16px rgba(37, 99, 235, 0.36);
}

.uce-btn.uce-ctx-glow--supplement {
  box-shadow: 0 0 0 1px rgba(245, 158, 11, 0.52), 0 0 16px rgba(217, 119, 6, 0.4);
}

.uce-btn.uce-ctx-glow--final_bill {
  box-shadow: 0 0 0 1px rgba(34, 197, 94, 0.42), 0 0 14px rgba(21, 128, 61, 0.34);
}

.uce-btn.uce-ctx-glow--print {
  box-shadow: 0 0 0 1px rgba(96, 165, 250, 0.45), 0 0 18px rgba(59, 130, 246, 0.42);
}

.uce-btn.uce-ctx-glow--tesla {
  box-shadow: 0 0 0 1px rgba(244, 63, 94, 0.38), 0 0 14px rgba(225, 29, 72, 0.3);
}

.uce-btn.uce-ctx-glow--parts {
  box-shadow: 0 0 0 1px rgba(168, 85, 247, 0.4), 0 0 14px rgba(147, 51, 234, 0.32);
}

.uce-capture-mode {
  position: absolute;
  right: -1px;
  bottom: -1px;
  font-size: 8px;
  line-height: 1;
  pointer-events: none;
  z-index: 2;
  filter: drop-shadow(0 0 1px rgba(0, 0, 0, 0.55));
}

.uce-btn--opportunity-likely {
  outline: 1px solid rgba(250, 204, 21, 0.55);
  outline-offset: 1px;
}

.uce-btn--candidate-uncertainty {
  opacity: 0.88;
  filter: saturate(0.92);
}

.uce-btn.uce-ctx-glow--candidate-soft {
  box-shadow: 0 0 0 1px rgba(148, 163, 184, 0.35), 0 0 10px rgba(100, 116, 139, 0.22);
}

.uce-btn.uce-btn--context-glow.uce-glow-strength--low {
  filter: brightness(0.96) saturate(0.94);
}

.uce-btn.uce-btn--context-glow.uce-glow-strength--mid {
  filter: brightness(1) saturate(1);
}

.uce-btn.uce-btn--context-glow.uce-glow-strength--high {
  filter: brightness(1.04) saturate(1.08);
}

@keyframes uce-cue-pulse-once {
  0% {
    box-shadow: 0 0 0 0 rgba(96, 165, 250, 0);
    transform: scale(1);
  }
  40% {
    box-shadow: 0 0 0 3px rgba(96, 165, 250, 0.42), 0 0 20px rgba(59, 130, 246, 0.38);
    transform: scale(1.03);
  }
  100% {
    box-shadow: 0 0 0 0 rgba(96, 165, 250, 0);
    transform: scale(1);
  }
}

.uce-btn.uce-btn--cue-pulse {
  animation: uce-cue-pulse-once 0.88s ease forwards;
}

@keyframes uce-breathe {
  0% { transform: scale(1); }
  50% { transform: scale(1.015); }
  100% { transform: scale(1); }
}

.uce-capture-wrap .uce-icon {
  width: 16px;
  height: 16px;
  pointer-events: none;
  flex-shrink: 0;
}

.uce-capture-wrap .flash {
  position: absolute;
  inset: 0;
  z-index: 1;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0);
  pointer-events: none;
}

.flash.on {
  animation: flash 180ms ease;
}

@keyframes flash {
  0% { background: rgba(255, 255, 255, 0.88); }
  100% { background: rgba(255, 255, 255, 0); }
}

.toast {
  position: fixed;
  left: 8px;
  top: 8px;
  right: 8px;
  z-index: 10;
  display: block;
  width: auto;
  max-width: none;
  min-width: 0;
  height: auto;
  min-height: 0;
  padding: 10px 14px;
  border-radius: 8px;
  font-family: "Segoe UI", sans-serif;
  font-size: 13px;
  line-height: 1.45;
  color: #fff;
  background: rgba(17, 24, 39, 0.96);
  white-space: normal;
  word-wrap: break-word;
  overflow-wrap: anywhere;
  word-break: break-word;
  overflow: visible;
  opacity: 0;
  transform: translateY(2px);
  transition: opacity 140ms ease, transform 140ms ease;
  pointer-events: none;
  box-sizing: border-box;
  -webkit-font-smoothing: antialiased;
}

.uce-toast-brand {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  color: rgba(226, 232, 240, 0.92);
  margin-bottom: 6px;
  line-height: 1.2;
}

.uce-toast-msg {
  font-size: 13px;
  font-weight: 400;
  line-height: 1.45;
  white-space: pre-line;
  word-wrap: break-word;
  overflow-wrap: anywhere;
  word-break: break-word;
}

.toast.show {
  opacity: 1;
  transform: translateY(0);
  pointer-events: auto;
}

.toast.success {
  background: rgba(22, 163, 74, 0.95);
}

.toast.error {
  background: rgba(220, 38, 38, 0.95);
}

.toast.warn {
  background: rgba(202, 138, 4, 0.96);
}

.uce-ro-panel {
  border-top: 1px solid rgba(148, 163, 184, 0.22);
  padding-top: 8px;
  font-size: 11px;
  line-height: 1.4;
}

.uce-ro-badge {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 6px;
  font-weight: 700;
  font-size: 11px;
  margin-bottom: 6px;
}

.uce-ro-badge.green {
  background: rgba(34, 197, 94, 0.25);
  color: #bbf7d0;
}

.uce-ro-badge.yellow {
  background: rgba(234, 179, 8, 0.25);
  color: #fef08a;
}

.uce-ro-badge.red {
  background: rgba(239, 68, 68, 0.28);
  color: #fecaca;
}

.uce-ro-row {
  margin: 3px 0;
  padding: 2px 0 2px 8px;
  border-left: 2px solid rgba(148, 163, 184, 0.35);
}

.uce-ro-chip {
  font-size: 9px;
  font-weight: 700;
  padding: 1px 5px;
  border-radius: 4px;
  margin-left: 6px;
  vertical-align: middle;
}

.uce-ro-chip.crit {
  background: rgba(239, 68, 68, 0.4);
}

.uce-ro-chip.seen {
  background: rgba(59, 130, 246, 0.4);
}

.uce-ro-hint {
  margin: 6px 0 0;
  color: #94a3b8;
  font-size: 11px;
}

.uce-ro-action {
  margin-top: 8px;
  width: 100%;
  padding: 7px 8px;
  border-radius: 6px;
  border: none;
  background: #2563eb;
  color: #fff;
  font-size: 11px;
  font-weight: 600;
  cursor: pointer;
  font-family: "Segoe UI", sans-serif;
}

.uce-ro-action:hover {
  filter: brightness(1.08);
}

.uce-tenant-setup {
  position: fixed;
  inset: 0;
  z-index: 200;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 12px;
  background: rgba(15, 23, 42, 0.72);
  box-sizing: border-box;
}

.uce-tenant-setup[hidden] {
  display: none !important;
}

.uce-tenant-setup-inner {
  width: 100%;
  max-width: 380px;
  padding: 18px 20px;
  border-radius: 10px;
  background: rgba(30, 41, 59, 0.98);
  border: 1px solid rgba(148, 163, 184, 0.35);
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.35);
}

.uce-tenant-setup-title {
  margin: 0 0 6px;
  font-size: 15px;
  font-weight: 700;
  font-family: "Segoe UI", sans-serif;
  color: #f8fafc;
}

.uce-tenant-setup-hint {
  margin: 0 0 12px;
  font-size: 12px;
  line-height: 1.45;
  color: #94a3b8;
  font-family: "Segoe UI", sans-serif;
}

.uce-tenant-input {
  width: 100%;
  box-sizing: border-box;
  padding: 10px 12px;
  border-radius: 6px;
  border: 1px solid rgba(148, 163, 184, 0.45);
  background: rgba(15, 23, 42, 0.9);
  color: #f1f5f9;
  font-size: 13px;
  font-family: ui-monospace, "Cascadia Code", Consolas, monospace;
  margin-bottom: 8px;
}

.uce-tenant-input:focus {
  outline: none;
  border-color: #3b82f6;
}

.uce-tenant-error {
  margin: 0 0 10px;
  font-size: 12px;
  color: #fca5a5;
  font-family: "Segoe UI", sans-serif;
}

.uce-tenant-save {
  width: 100%;
  padding: 10px 14px;
  border: none;
  border-radius: 8px;
  font-size: 13px;
  font-weight: 600;
  font-family: "Segoe UI", sans-serif;
  color: #fff;
  background: linear-gradient(180deg, #3b82f6 0%, #2563eb 100%);
  cursor: pointer;
}

.uce-tenant-save:hover {
  filter: brightness(1.06);
}

#app.uce-tenant-setup-open #uceDock .uce-toolbar,
#app.uce-tenant-setup-open .uce-debug-sheet,
#app.uce-tenant-setup-open .toast,
#app.uce-tenant-setup-open .uce-ro-toggle,
#app.uce-tenant-setup-open .uce-blocking-banner {
  pointer-events: none;
}

/* Production RO panel (no debug text) — shown on demand only */
.uce-prod-shell {
  font-size: 12px;
  line-height: 1.45;
  color: #f1f5f9;
  max-width: 100%;
}

.uce-prod-head {
  font-weight: 700;
  font-size: 13px;
  margin-bottom: 8px;
  color: #f8fafc;
}

.uce-prod-row {
  margin: 4px 0 10px;
  display: flex;
  align-items: baseline;
  gap: 8px;
  flex-wrap: wrap;
}

.uce-prod-k {
  color: #94a3b8;
  font-weight: 600;
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.uce-prod-v {
  font-weight: 700;
  font-size: 13px;
  color: #f1f5f9;
}

.uce-prod-v.uce-prod-ok {
  color: #86efac;
}

.uce-prod-v.uce-prod-bad {
  color: #fca5a5;
}

.uce-prod-v.uce-prod-muted {
  color: #94a3b8;
  font-weight: 600;
}

.uce-prod-section {
  margin-top: 10px;
}

.uce-prod-section .uce-prod-k {
  display: block;
  margin-bottom: 4px;
}

.uce-prod-subhint {
  font-size: 10px;
  color: #64748b;
  font-weight: 400;
  text-transform: none;
  letter-spacing: 0;
  margin: 0 0 6px;
  line-height: 1.35;
}

.uce-prod-list {
  margin: 0;
  padding-left: 18px;
  color: #cbd5e1;
  font-size: 11px;
}

.uce-prod-li-sub {
  display: block;
  font-size: 10px;
  color: #94a3b8;
  font-weight: 400;
  margin-top: 2px;
}

.uce-debug-sheet.uce-debug-sheet--prod .uce-prod-li-sub {
  color: rgba(167, 243, 208, 0.78);
}

.uce-prod-empty {
  color: #64748b;
  font-size: 11px;
  font-style: italic;
}

.uce-prod-msg {
  margin: 0;
  color: #94a3b8;
  font-size: 11px;
  line-height: 1.5;
}

.uce-prod-hint {
  margin: 8px 0 0;
  color: #64748b;
  font-size: 10px;
  line-height: 1.45;
  border-top: 1px solid rgba(148, 163, 184, 0.2);
  padding-top: 8px;
}

.uce-prod-raw-wrap {
  margin-top: 10px;
  font-size: 10px;
  color: #94a3b8;
}

.uce-prod-raw-wrap summary {
  cursor: pointer;
  user-select: none;
  color: #cbd5e1;
}

.uce-prod-raw-json {
  margin: 6px 0 0;
  padding: 8px;
  max-height: 180px;
  overflow: auto;
  font-size: 9px;
  line-height: 1.35;
  white-space: pre-wrap;
  word-break: break-word;
  background: rgba(15, 23, 42, 0.85);
  border: 1px solid rgba(148, 163, 184, 0.25);
  border-radius: 6px;
  color: #e2e8f0;
}

.uce-prod-actions {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin-top: 14px;
}

.uce-prod-btn {
  width: 100%;
  padding: 10px 12px;
  border-radius: 8px;
  border: none;
  font-size: 12px;
  font-weight: 600;
  font-family: "Segoe UI", sans-serif;
  cursor: pointer;
}

.uce-prod-btn-primary {
  background: linear-gradient(180deg, #3b82f6 0%, #2563eb 100%);
  color: #fff;
}

.uce-prod-btn-primary:hover {
  filter: brightness(1.06);
}

.uce-prod-btn-secondary {
  background: rgba(51, 65, 85, 0.85);
  color: #e2e8f0;
  border: 1px solid rgba(148, 163, 184, 0.35);
}

.uce-prod-btn-secondary:hover {
  background: rgba(71, 85, 105, 0.95);
}

/* Compact staff checklist (default RO panel — not diagnostic) */
.uce-prod-shell--compact {
  line-height: 1.35;
}

.uce-prod-top {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 6px;
  margin: 0 0 6px;
}

.uce-prod-ro-line {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  width: 100%;
}

.uce-prod-ro-num {
  font-weight: 800;
  font-size: 15px;
  color: #f8fafc;
  letter-spacing: -0.02em;
}

.uce-prod-chip-row {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  align-items: center;
}

.uce-prod-chip {
  display: inline-block;
  padding: 2px 7px;
  border-radius: 999px;
  font-size: 10px;
  font-weight: 700;
  line-height: 1.2;
  color: #ecfdf5;
  background: rgba(15, 23, 42, 0.55);
  border: 1px solid rgba(167, 243, 208, 0.28);
  white-space: nowrap;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
}

.uce-prod-chip--ok {
  color: #bbf7d0;
  border-color: rgba(34, 197, 94, 0.45);
  background: rgba(22, 101, 52, 0.35);
}

.uce-prod-chip--warn {
  color: #fef3c7;
  border-color: rgba(251, 191, 36, 0.45);
  background: rgba(120, 53, 15, 0.4);
}

.uce-prod-chip--bad {
  color: #fecaca;
  border-color: rgba(248, 113, 113, 0.4);
  background: rgba(127, 29, 29, 0.35);
}

.uce-prod-mini {
  margin: 0;
  font-size: 10px;
  color: rgba(167, 243, 208, 0.82);
  line-height: 1.35;
}

.uce-prod-block {
  margin-top: 6px;
}

.uce-prod-block-title {
  font-size: 10px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  color: rgba(167, 243, 208, 0.9);
  margin: 0 0 4px;
}

.uce-prod-checklist {
  margin: 0;
  padding-left: 16px;
  color: #ecfdf5;
  font-size: 11px;
  font-weight: 500;
}

.uce-prod-checklist li {
  margin: 2px 0;
}

.uce-prod-next {
  font-size: 12px;
  font-weight: 700;
  color: #fef08a;
  margin: 0;
}

.uce-prod-next.uce-prod-next--none {
  color: rgba(167, 243, 208, 0.75);
  font-weight: 600;
}

.uce-prod-next.uce-prod-next--maybe {
  color: rgba(253, 224, 71, 0.95);
}

.uce-prod-do {
  margin: 4px 0 0;
  font-size: 10px;
  font-weight: 600;
  color: rgba(224, 242, 231, 0.88);
}

.uce-prod-actions--compact {
  margin-top: 10px;
  gap: 6px;
}

.uce-prod-more-details {
  margin-top: 8px;
  padding-top: 6px;
  border-top: 1px solid rgba(34, 197, 94, 0.22);
}

.uce-prod-more-details > summary {
  list-style: none;
  cursor: pointer;
  font-size: 11px;
  font-weight: 600;
  color: rgba(167, 243, 208, 0.95);
  user-select: none;
}

.uce-prod-more-details > summary::-webkit-details-marker {
  display: none;
}

.uce-prod-more-details > summary::after {
  content: " ▼";
  font-size: 9px;
  opacity: 0.75;
}

.uce-prod-more-details[open] > summary::after {
  content: " ▲";
}

.uce-prod-detail-block {
  margin-top: 8px;
}

.uce-prod-detail-block > .uce-prod-block-title {
  margin-bottom: 3px;
}

.uce-prod-more-details .uce-prod-raw-json {
  max-height: 140px;
}

/* Health dot hidden — status still drives banners/modals; hover was extra chrome. */
.uce-health {
  display: none !important;
  width: 10px;
  height: 10px;
  border-radius: 50%;
  flex-shrink: 0;
  align-self: center;
  background: #22c55e;
  box-shadow: 0 0 0 1px rgba(15, 23, 42, 0.35);
}
.uce-health--warn {
  background: #f59e0b;
}
.uce-health--bad {
  background: #ef4444;
}

.uce-blocking-banner {
  box-sizing: border-box;
  width: max-content;
  max-width: min(380px, calc(100vw - 8px));
  margin: 0 0 4px;
  padding: 8px 10px;
  border-radius: 6px;
  background: linear-gradient(180deg, #b91c1c 0%, #991b1b 100%);
  border: 1px solid rgba(254, 202, 202, 0.45);
  color: #fef2f2;
  font-size: 11px;
  font-weight: 600;
  line-height: 1.35;
  text-align: center;
  font-family: "Segoe UI", sans-serif;
  flex-shrink: 0;
}

/* When RO/debug panel is open, #app.uce-debug-open owns column layout — do not pin toolbar to bottom. */
#app.uce-blocking-banner-visible:not(.uce-debug-open):not(.uce-toast-open) {
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  align-items: flex-start;
  width: fit-content;
  height: fit-content;
  max-width: 100vw;
  padding-bottom: 4px;
  padding-top: 2px;
}

#app.uce-blocking-banner-visible:not(.uce-debug-open) #uceDock {
  position: relative;
  flex-shrink: 0;
}
#app.uce-blocking-banner-visible:not(.uce-debug-open) #uceDock .uce-toolbar {
  position: relative;
  left: auto;
  top: auto;
  transform: none;
  flex-shrink: 0;
}

.uce-printer-severe-modal {
  position: fixed;
  inset: 0;
  z-index: 190;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 12px;
  background: rgba(15, 23, 42, 0.72);
  box-sizing: border-box;
}

.uce-printer-severe-modal[hidden] {
  display: none !important;
}

.uce-printer-severe-inner {
  width: 100%;
  max-width: 380px;
  padding: 18px 20px;
  border-radius: 10px;
  background: rgba(30, 41, 59, 0.98);
  border: 1px solid rgba(248, 113, 113, 0.5);
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.35);
}

.uce-printer-severe-title {
  margin: 0 0 6px;
  font-size: 15px;
  font-weight: 700;
  font-family: "Segoe UI", sans-serif;
  color: #fecaca;
}

.uce-printer-severe-hint {
  margin: 0 0 12px;
  font-size: 12px;
  line-height: 1.45;
  color: #94a3b8;
  font-family: "Segoe UI", sans-serif;
}

.uce-printer-severe-ok {
  width: 100%;
  padding: 10px 14px;
  border: none;
  border-radius: 8px;
  font-size: 13px;
  font-weight: 600;
  font-family: "Segoe UI", sans-serif;
  color: #fff;
  background: linear-gradient(180deg, #dc2626 0%, #b91c1c 100%);
  cursor: pointer;
}

.uce-printer-severe-ok:hover {
  filter: brightness(1.06);
}

#app.uce-printer-severe-open #uceDock .uce-toolbar,
#app.uce-printer-severe-open .uce-debug-sheet,
#app.uce-printer-severe-open .toast,
#app.uce-printer-severe-open .uce-ro-toggle,
#app.uce-printer-severe-open .uce-blocking-banner,
#app.uce-printer-severe-open .uce-qa-bar {
  pointer-events: none;
}

/* QA / automation: CCC Change Request Word close (Ctrl+Shift+W). Opt-in: dev, VITE_UCE_QA=1, or localStorage uce_qa_tools=1 */
.uce-qa-bar {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  padding: 2px 4px;
  border-radius: 4px;
  background: rgba(15, 23, 42, 0.82);
  border: 1px solid rgba(148, 163, 184, 0.35);
  pointer-events: auto;
  max-width: min(280px, 92vw);
  box-sizing: border-box;
}

.uce-qa-bar[hidden] {
  display: none !important;
}

.uce-qa-bar-label {
  font-size: 9px;
  font-weight: 700;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  color: rgba(226, 232, 240, 0.85);
  font-family: "Segoe UI", sans-serif;
}

.uce-qa-bar-btn {
  margin: 0;
  padding: 3px 8px;
  border: none;
  border-radius: 4px;
  font-size: 10px;
  font-weight: 600;
  font-family: "Segoe UI", sans-serif;
  color: #f8fafc;
  background: linear-gradient(180deg, #475569 0%, #334155 100%);
  cursor: pointer;
  white-space: nowrap;
}

.uce-qa-bar-btn:hover {
  filter: brightness(1.08);
}

.uce-qa-bar-btn:active {
  filter: brightness(0.94);
}

</style>

<div id="uceBlockingBanner" class="uce-blocking-banner" hidden role="alert" aria-live="assertive">
  <span class="uce-blocking-banner-text">⚠️ FileWisely needs attention — documents may not be captured</span>
</div>
<div id="uceDebugSheet" class="uce-debug-sheet" hidden aria-hidden="true"></div>
<div id="uceDock">
<div class="uce-toolbar">
  <div class="uce-capture-wrap">
    <button type="button" class="uce-btn" id="uceBtn" aria-label="Capture" title="Capture current screen or document">
      <svg class="uce-icon" viewBox="0 0 24 24" aria-hidden="true">
        <path fill="currentColor" d="M9.8 4.7a1 1 0 0 1 .84-.47h2.72a1 1 0 0 1 .84.47l.74 1.13h2.31A2.75 2.75 0 0 1 20 8.58v7.67A2.75 2.75 0 0 1 17.25 19H6.75A2.75 2.75 0 0 1 4 16.25V8.58a2.75 2.75 0 0 1 2.75-2.75h2.31l.74-1.13Zm2.2 3.1a4.2 4.2 0 1 0 0 8.4 4.2 4.2 0 0 0 0-8.4Zm0 1.8a2.4 2.4 0 1 1 0 4.8 2.4 2.4 0 0 1 0-4.8Z"/>
      </svg>
      <span class="uce-capture-mode" id="uceCaptureModeBadge" hidden aria-hidden="true"></span>
    </button>
    <div class="flash" id="flash"></div>
  </div>
  <button type="button" class="uce-ro-toggle" id="uceRoPanelBtn" aria-label="Repair order status" title="RO status (FileWisely) — click again to close. Opening the panel keeps the checklist visible while you capture.">
    RO
  </button>
  <button type="button" class="uce-train" id="uceTrainBtn" aria-label="Train workflow for the active window. Alt+click: cycle target (CCC, Tesla EPC, …). Shift+click: forget training. Ctrl+Shift+click: exclude. Ctrl+click: clear exclude." title="Train — uses target workflow (Alt+click to cycle: CCC, Tesla EPC, PartsTrader, OPS Trax, Generic). Shift+click: forget. Ctrl+Shift+click: exclude. Ctrl+click: clear exclude.">
    T
  </button>
  <button type="button" class="uce-settings" id="uceSettingsBtn" aria-label="Toggle training button" title="Show or hide the training (T) button for onboarding">
    ⚙
  </button>
  <span class="uce-health" id="uceHealthStrip" role="status" aria-live="polite" title="System health"></span>
</div>
  <div id="uceQaBar" class="uce-qa-bar" hidden aria-label="QA automation">
    <span class="uce-qa-bar-label">QA</span>
    <button type="button" class="uce-qa-bar-btn" id="uceQaCloseWordBtn" title="Same as Ctrl+Shift+W — close foreground Word when Change Request flow is armed.">
      Close Word (CCC CR)
    </button>
  </div>
</div>
<div class="toast" id="toast" role="status" aria-live="polite"></div>
<div id="uceTenantSetup" class="uce-tenant-setup" hidden>
  <div class="uce-tenant-setup-inner">
    <h2 class="uce-tenant-setup-title">Connect FileWisely</h2>
    <p class="uce-tenant-setup-hint">Paste your <strong>business ID</strong> (UUID) from FileWisely (Advanced Settings in the web app). Required to upload captures.</p>
    <input type="text" id="uceTenantInput" class="uce-tenant-input" spellcheck="false" autocomplete="off" placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" aria-label="Business ID" />
    <p id="uceTenantError" class="uce-tenant-error" hidden role="alert"></p>
    <button type="button" class="uce-tenant-save" id="uceTenantSaveBtn">Continue</button>
  </div>
</div>
<div id="ucePrinterSevereModal" class="uce-printer-severe-modal" hidden>
  <div class="uce-printer-severe-inner">
    <h2 class="uce-printer-severe-title">Printer issue detected</h2>
    <p class="uce-printer-severe-hint">UCE is attempting automatic repair. If this persists, run the FileWisely installer or reinstall the PDF printer.</p>
    <button type="button" class="uce-printer-severe-ok" id="ucePrinterSevereOk">OK</button>
  </div>
</div>
`;

const uceBtn = document.getElementById("uceBtn");
const uceCaptureModeBadge = document.getElementById("uceCaptureModeBadge");
const uceDockEl = document.getElementById("uceDock");
const uceRoPanelBtn = document.getElementById("uceRoPanelBtn");
const uceTrainBtn = document.getElementById("uceTrainBtn");
const uceSettingsBtn = document.getElementById("uceSettingsBtn");
const uceHealthStrip = document.getElementById("uceHealthStrip");
const uceBlockingBanner = document.getElementById("uceBlockingBanner");
const ucePrinterSevereModal = document.getElementById("ucePrinterSevereModal");
const flashEl = document.getElementById("flash");
const toastEl = document.getElementById("toast");
const debugSheetEl = document.getElementById("uceDebugSheet");
const uceQaBar = document.getElementById("uceQaBar");
const uceQaCloseWordBtn = document.getElementById("uceQaCloseWordBtn");

/** Blocking health banner visible — must exist before early `updateRoToolbarLabel` → `getCompactWindowSize`. */
let healthBannerVisible = false;

function saveToLocalLog(event) {
  try {
    const raw = localStorage.getItem(UCE_EVENT_LOG_KEY);
    const arr = raw ? JSON.parse(raw) : [];
    if (!Array.isArray(arr)) return;
    arr.push(event);
    while (arr.length > UCE_EVENT_LOG_MAX) arr.shift();
    localStorage.setItem(UCE_EVENT_LOG_KEY, JSON.stringify(arr));
  } catch (e) {
    console.warn("[UCE] saveToLocalLog:", e);
  }
}

function logEvent(type, message) {
  const event = {
    type: String(type ?? "unknown"),
    message: String(message ?? ""),
    timestamp: Date.now(),
  };
  saveToLocalLog(event);
  console.info(`[UCE event] ${event.type}:`, event.message);
}

function getUceEventLog() {
  try {
    const raw = localStorage.getItem(UCE_EVENT_LOG_KEY);
    const arr = raw ? JSON.parse(raw) : [];
    return Array.isArray(arr) ? arr : [];
  } catch {
    return [];
  }
}

currentRoLive = loadPersistedCurrentRo();
updateRoToolbarLabel();

/** Standard UUID string (any version). */
function isValidUuid(value) {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
    String(value).trim()
  );
}

/**
 * First launch: no tenant in uce-tenant.json and no VITE_UCE_BUSINESS_ID — block until saved.
 */
async function showTenantSetupDialog() {
  const overlay = document.getElementById("uceTenantSetup");
  const input = document.getElementById("uceTenantInput");
  const errEl = document.getElementById("uceTenantError");
  const btn = document.getElementById("uceTenantSaveBtn");
  if (!overlay || !input || !errEl || !btn) {
    throw new Error("UCE: tenant setup DOM missing");
  }

  overlay.hidden = false;
  appEl.classList.add("uce-tenant-setup-open");
  try {
    await invoke("uce_set_overlay_logical_size", { width: 420, height: 280 });
    await delayToastLayout(80);
  } catch (e) {
    console.error("tenant setup resize:", e);
  }
  input.value = "";
  errEl.hidden = true;
  errEl.textContent = "";
  input.focus();

  return new Promise((resolve) => {
    const submit = async () => {
      const v = input.value.trim();
      if (!isValidUuid(v)) {
        errEl.textContent =
          "Enter a valid business ID (UUID). Copy it from FileWisely → Advanced Settings.";
        errEl.hidden = false;
        return;
      }
      errEl.hidden = true;
      try {
        await invoke("save_tenant_business_id", { business_id: v });
        await initTenantContext();
        overlay.hidden = true;
        appEl.classList.remove("uce-tenant-setup-open");
        await setCompactWindowSize();
        btn.removeEventListener("click", onClick);
        input.removeEventListener("keydown", onKey);
        resolve();
      } catch (e) {
        errEl.textContent =
          typeof e === "string" ? e : e?.message || String(e);
        errEl.hidden = false;
      }
    };
    function onClick() {
      void submit();
    }
    function onKey(e) {
      if (e.key === "Enter") void submit();
    }
    btn.addEventListener("click", onClick);
    input.addEventListener("keydown", onKey);
  });
}

const TRAIN_BUTTON_VISIBLE_KEY = "uce_train_button_visible";

/** `null` in storage = show T (backward compatible). Set `"0"` to hide during steady-state. */
function getTrainButtonVisible() {
  const v = localStorage.getItem(TRAIN_BUTTON_VISIBLE_KEY);
  if (v === null) return true;
  return v === "1" || v === "true";
}

function setTrainButtonVisible(visible) {
  localStorage.setItem(TRAIN_BUTTON_VISIBLE_KEY, visible ? "1" : "0");
  applyTrainButtonVisibility();
  void setCompactWindowSize();
}

function applyTrainButtonVisibility() {
  const on = getTrainButtonVisible();
  uceTrainBtn.classList.toggle("uce-hidden", !on);
  uceTrainBtn.setAttribute("aria-hidden", on ? "false" : "true");
  uceSettingsBtn.title = on
    ? "Hide training (T) button — use when onboarding is done"
    : "Show training (T) button — use for a new shop, new system, or new sites";
  uceSettingsBtn.setAttribute(
    "aria-label",
    on ? "Hide CCC training button" : "Show CCC training button"
  );
}

/** Dev, `VITE_UCE_QA=1`, or `localStorage uce_qa_tools=1` — shows the QA dock row (automation API is always available). */
function isUceQaAutomationEnabled() {
  try {
    if (import.meta.env.DEV) return true;
    if (String(import.meta.env.VITE_UCE_QA ?? "").trim() === "1") return true;
    if (localStorage.getItem("uce_qa_tools") === "1") return true;
  } catch {
    /* ignore */
  }
  return false;
}

function initUceQaAutomation() {
  if (!uceQaBar || !uceQaCloseWordBtn) return;

  window.__uceQa = {
    ...window.__uceQa,
    closeWord: async () => {
      try {
        const msg = await invoke("uce_ccc_cr_manual_close_word");
        showToast(String(msg ?? "Word close requested"), "success", 3200);
        return { ok: true, message: msg };
      } catch (e) {
        const t = typeof e === "string" ? e : e?.message ?? String(e);
        showToast(t, "error", 5000);
        return { ok: false, error: t };
      }
    },
  };

  uceQaCloseWordBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    void window.__uceQa.closeWord();
  });

  if (isUceQaAutomationEnabled()) {
    uceQaBar.hidden = false;
    void setCompactWindowSize();
  }

  new ResizeObserver(() => {
    if (!uceQaBar.hidden && shouldMeasureDomForCompactWindow()) {
      scheduleCompactResizeFromDockMeasure();
    }
  }).observe(uceQaBar);
}

/**
 * Toolbar width must match the real flex row (RO label grows with digit count).
 * A fixed ~24px RO assumption caused horizontal overflow → WebView scrollbars.
 */
function estimateRoToggleWidthPx() {
  const t = (uceRoPanelBtn?.textContent || "RO").trim() || "RO";
  return Math.max(24, 12 + Math.ceil(t.length * 7.5));
}

function shouldMeasureDomForCompactWindow() {
  return (
    !appEl.classList.contains("uce-toast-open") &&
    !appEl.classList.contains("uce-debug-open") &&
    !appEl.classList.contains("uce-tenant-setup-open") &&
    !appEl.classList.contains("uce-printer-severe-open")
  );
}

/** `localStorage.setItem("uce_overlay_hit_debug", "1")` → console logs window vs DOM bounds. */
function logOverlayHitDebug(logicalW, logicalH, phase) {
  if (localStorage.getItem("uce_overlay_hit_debug") !== "1") return;
  void uceDockEl?.offsetWidth;
  const dock = uceDockEl?.getBoundingClientRect();
  const btn = uceBtn?.getBoundingClientRect();
  const root = appEl?.getBoundingClientRect();
  console.info(`[UCE overlay hit debug] ${phase}`, {
    logicalTarget: { w: logicalW, h: logicalH },
    appBoundingCss: root
      ? { w: root.width, h: root.height, x: root.x, y: root.y }
      : null,
    dockBoundingCss: dock
      ? { w: dock.width, h: dock.height, x: dock.x, y: dock.y }
      : null,
    buttonBoundingCss: btn
      ? { w: btn.width, h: btn.height, x: btn.x, y: btn.y }
      : null,
  });
}

function getCompactWindowSize() {
  if (shouldMeasureDomForCompactWindow() && uceDockEl) {
    void uceDockEl.offsetWidth;
    void appEl.offsetWidth;
    let w = 0;
    let h = 0;
    if (
      healthBannerVisible &&
      uceBlockingBanner &&
      !uceBlockingBanner.hidden
    ) {
      void uceBlockingBanner.offsetWidth;
      const br = uceBlockingBanner.getBoundingClientRect();
      const dr = uceDockEl.getBoundingClientRect();
      w = Math.max(Math.ceil(br.width), Math.ceil(dr.width));
      h = Math.ceil(br.height + dr.height + 6);
    } else {
      const dr = uceDockEl.getBoundingClientRect();
      w = Math.ceil(dr.width);
      h = Math.ceil(dr.height);
    }
    if (w >= 8 && h >= 8) {
      const out = {
        width: Math.max(38, w),
        height: Math.max(38, h),
      };
      logOverlayHitDebug(out.width, out.height, "getCompact(dom-measure)");
      return out;
    }
  }

  const roW = estimateRoToggleWidthPx();
  const gap = 2;
  const pad = 0;
  const rowH = 38;
  const h =
    rowH +
    (healthBannerVisible ? HEALTH_BANNER_HEIGHT_LOGICAL : 0);
  const capture = 38;
  const train = 22;
  const settings = 22;
  const w =
    capture +
    gap +
    roW +
    gap +
    (getTrainButtonVisible() ? train + gap : 0) +
    settings +
    pad;
  const out = { width: w, height: h };
  logOverlayHitDebug(out.width, out.height, "getCompact(formula-fallback)");
  return out;
}

/** After this hold, OS drags the frame (`start_window_drag`); release → snap + save or capture click. */
let pointerIsDown = false;
let dockNativeDragStarted = false;
let dockDragTimer = null;
let pointerShiftPressed = false;
let busy = false;
let toastTimer = null;
let contextPollTimer = null;
let autoPdfTimer = null;
let currentUiState = "quiet";
let lastPolledContext = null;
let lastUploadDebug = null;
let autoPdfSinceUnixMs = Date.now();
let lastAutoPdfKey = "";
let lastAutoPdfUploadAt = 0;
/** Any successful backend upload (for health tooltip). */
let lastSuccessfulUploadAt = 0;
let lastPrinterRepairAttemptAt = 0;
let lastPrinterHealthFetch = 0;
/** Cached from `uce_check_filewisely_printer` (refreshed on interval / self-heal). */
let healthPrinterExact = false;
/** First time printer missing / core unhealthy (for sustained alerts). */
let printerMissingSince = null;
let coreUnhealthySince = null;
let lastEdgePrinterMissing = false;
/** User dismissed the severe printer modal; show again only after printer recovers. */
let printerModalDismissedUntilOk = false;
let autoPdfBusy = false;
/** Set when an incoming debounced run was skipped because a previous auto-PDF run was still active. */
let autoPdfIncomingPending = false;
let incomingAutoPdfDebounceTimer = null;
/** True while right button is held for “peek” debug (release to close). */
let rightPeekActive = false;
/** Which side panel is open: `debug` (Ctrl+right-click) or `production` (RO button / right-click). */
let peekSidePanelMode = null;
/** Session fingerprints for PDF auto-upload duplicate suppression (`path|mtime`). */
const uploadedPdfFingerprints = new Set();
/** RO status cache: one entry; refreshed on context + 60s poll. */
let roStatusCache = { ro: "", data: null, fetchedAt: 0 };
let roStatusBackgroundTimer = null;
/** Logical/CSS pixels — must match setSize(LogicalSize) so DPI scaling matches the WebView. */
const COMPACT_WINDOW_SIZE = { width: 58, height: 38 };
const TOAST_MAX_W = 680;
/** Must stay ≤ tauri.conf `maxHeight` (native clamp). */
const TOAST_MAX_H = 2000;
/** Extra logical px below toast bottom so WebView does not clip the last line(s). */
const TOAST_VIEWPORT_BOTTOM_MARGIN = 52;
/** Space for capture + train + gear row pinned under the toast while a message is visible. */
const TOAST_TOOLBAR_ROW_RESERVE = 46;
const WINDOW_MAX = new LogicalSize(700, 2000);
const COMPACT_MIN = new LogicalSize(38, 38);
const TOAST_PANEL_MIN = new LogicalSize(320, 120);

let toastFitDebounceTimer = null;
let compactDockResizeDebounceTimer = null;

function scheduleCompactResizeFromDockMeasure() {
  if (!shouldMeasureDomForCompactWindow()) return;
  if (compactDockResizeDebounceTimer)
    clearTimeout(compactDockResizeDebounceTimer);
  compactDockResizeDebounceTimer = setTimeout(() => {
    compactDockResizeDebounceTimer = null;
    void setCompactWindowSize();
  }, 100);
}

async function setCompactWindowSize() {
  try {
    const s = getCompactWindowSize();
    const w = Math.round(s.width);
    const h = Math.round(s.height);
    await appWindow.setResizable(true);
    await appWindow.setMinSize(COMPACT_MIN);
    await appWindow.setMaxSize(WINDOW_MAX);
    // Same path as toast / RO peek — WebView2 + JS setSize alone can leave a tall window after peek.
    await invoke("uce_set_overlay_logical_size", { width: w, height: h });
    await appWindow.setResizable(false);
    if (localStorage.getItem("uce_overlay_hit_debug") === "1") {
      try {
        const os = await appWindow.outerSize();
        void uceDockEl?.offsetWidth;
        const dock = uceDockEl?.getBoundingClientRect();
        const btn = uceBtn?.getBoundingClientRect();
        const root = appEl?.getBoundingClientRect();
        console.info("[UCE overlay hit debug] after setCompactWindowSize", {
          outerPhysicalPx: { w: os.width, h: os.height },
          logicalInvoked: { w, h },
          appBoundingCss: root
            ? { w: root.width, h: root.height }
            : null,
          dockBoundingCss: dock
            ? { w: dock.width, h: dock.height }
            : null,
          buttonBoundingCss: btn
            ? { w: btn.width, h: btn.height }
            : null,
        });
      } catch (_) {
        /* ignore */
      }
    }
  } catch (err) {
    console.error("setCompactWindowSize:", err);
  }
}

function delayToastLayout(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function setToastLayoutOpen(isOpen) {
  appEl.classList.toggle("uce-toast-open", isOpen);
}

/**
 * Measure height needed for the toast (logical px). Call only after the window is already tall
 * enough that layout is not clipped — otherwise numbers are wrong.
 */
function toastNeededHeightLogical() {
  void toastEl.offsetHeight;
  const r = toastEl.getBoundingClientRect();
  const top = Number.parseFloat(getComputedStyle(toastEl).top) || 8;
  const fromRect = Math.ceil(r.bottom);
  const fromScroll = Math.ceil(top + (toastEl.scrollHeight || r.height));
  const h =
    Math.max(fromRect, fromScroll) +
    TOAST_VIEWPORT_BOTTOM_MARGIN +
    TOAST_TOOLBAR_ROW_RESERVE;
  return Math.min(
    TOAST_MAX_H,
    Math.max(TOAST_PANEL_MIN.height, h)
  );
}

/**
 * Grow the window via native resize, then shrink to fit. Measuring while the window was still
 * ~44px tall made `getBoundingClientRect` / `scrollHeight` reflect the clipped box, not full text.
 */
async function resolveMonitorForClamp() {
  let m = await currentMonitor();
  if (!m) m = await primaryMonitor();
  if (!m) {
    const all = await availableMonitors();
    if (all.length) m = all[0];
  }
  return m;
}

/**
 * After widening the toast, the OS keeps the window’s top-left fixed, so a top-right pill grows
 * off the right edge of the monitor. Shift position so the full window stays in the work area.
 */
async function clampOverlayIntoWorkArea() {
  const mon = await resolveMonitorForClamp();
  if (!mon) return;

  const pos = await appWindow.outerPosition();
  const size = await appWindow.outerSize();
  const wa = mon.workArea;
  const margin = 10;

  const minX = wa.position.x + margin;
  const minY = wa.position.y + margin;
  const maxX = wa.position.x + wa.size.width - size.width - margin;
  const maxY = wa.position.y + wa.size.height - size.height - margin;

  let x = pos.x;
  let y = pos.y;
  if (maxX >= minX) {
    x = Math.min(Math.max(x, minX), maxX);
  } else {
    x = minX;
  }
  if (maxY >= minY) {
    y = Math.min(Math.max(y, minY), maxY);
  } else {
    y = minY;
  }

  if (x !== pos.x || y !== pos.y) {
    try {
      await appWindow.setPosition(new PhysicalPosition(x, y));
    } catch (e) {
      console.error("clampOverlayIntoWorkArea:", e);
    }
  }
}

async function fitWindowToToast() {
  if (!toastEl.className.includes("show")) return;
  /* Do not shrink the native window to “toast width” while the RO / debug side sheet is open — that
   * collapses fitRightPeekLayout and hides the checklist (felt like the panel closed while resolving documents). */
  if (appEl.classList.contains("uce-debug-open")) {
    return;
  }

  try {
    // Grow wide first so toast layout is accurate, then shrink width to content (avoids a 680px-wide invisible hit slab).
    await invoke("uce_set_overlay_logical_size", {
      width: TOAST_MAX_W,
      height: TOAST_MAX_H,
    });
    await delayToastLayout(110);
    void toastEl.offsetHeight;
    const toolbarEl = appEl.querySelector(".uce-toolbar");
    void toolbarEl?.offsetHeight;
    const tw = toastEl.getBoundingClientRect().width;
    const bw = toolbarEl?.getBoundingClientRect().width ?? 0;
    const panelW = Math.min(
      TOAST_MAX_W,
      Math.ceil(Math.max(tw, bw) + 16)
    );

    let h = toastNeededHeightLogical();
    await invoke("uce_set_overlay_logical_size", { width: panelW, height: h });

    await delayToastLayout(80);
    void toastEl.offsetHeight;
    const h2 = toastNeededHeightLogical();
    if (h2 > h + 2) {
      await invoke("uce_set_overlay_logical_size", { width: panelW, height: h2 });
    }
    await clampOverlayIntoWorkArea();
  } catch (err) {
    console.error("fitWindowToToast:", err);
  }
}

function scheduleFitWindowToToast() {
  if (toastFitDebounceTimer) clearTimeout(toastFitDebounceTimer);
  toastFitDebounceTimer = setTimeout(() => {
    toastFitDebounceTimer = null;
    fitWindowToToast();
  }, 100);
}

function getDeviceId() {
  const key = "uce_device_id";
  let id = localStorage.getItem(key);
  if (!id) {
    id = `uce-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
    localStorage.setItem(key, id);
  }
  return id;
}

/** Statuses FileWisely (or gateway) must send explicitly — we never infer “missing” from silence. */
const UCE_EXPLICIT_MISSING_STATUSES = new Set([
  "missing",
  "absent",
  "not_received",
  "required_missing",
  "critical_missing",
  "not_in_portal",
  "not_in_system",
]);

/** Map API-specific status flags to `captured` | `seen_not_captured` | `missing` | `not_captured_yet`. */
function canonicalizeRoItemStatus(raw) {
  if (!raw || typeof raw !== "object") return "not_captured_yet";
  if (raw.in_system === true || raw.in_filewisely === true || raw.captured === true) {
    return "captured";
  }
  const s = String(raw.status ?? raw.state ?? "")
    .toLowerCase()
    .replace(/\s+/g, "_")
    .replace(/-/g, "_");
  if (
    [
      "captured",
      "complete",
      "completed",
      "in_system",
      "present",
      "uploaded",
      "stored",
      "available",
      "in_portal",
      "inportal",
      "received",
      "done",
      "ingested",
    ].includes(s)
  ) {
    return "captured";
  }
  if (s === "seen_not_captured" || s === "seen" || s === "viewed") {
    return "seen_not_captured";
  }
  if (UCE_EXPLICIT_MISSING_STATUSES.has(s)) {
    return "missing";
  }
  return "not_captured_yet";
}

function normalizeRoStatusItem(raw) {
  if (raw == null) return null;
  if (typeof raw === "string") {
    return { label: raw, status: "not_captured_yet", critical: false };
  }
  if (typeof raw === "object") {
    const label =
      raw.label ||
      raw.name ||
      raw.title ||
      raw.document_type ||
      raw.document_name ||
      "Item";
    const status = canonicalizeRoItemStatus(raw);
    return {
      label: String(label),
      status,
      critical: !!raw.critical,
      captured_at:
        raw.captured_at ?? raw.uploaded_at ?? raw.created_at ?? raw.date ?? null,
    };
  }
  return null;
}

function normalizeInSystemEntry(raw) {
  if (raw == null) return null;
  if (typeof raw === "string") {
    return { label: raw.trim(), captured_at: null };
  }
  if (typeof raw === "object") {
    const label =
      raw.label ||
      raw.name ||
      raw.title ||
      raw.document_type ||
      raw.document_name ||
      raw.display_name ||
      raw.filename ||
      raw.file_name ||
      "Document";
    const captured_at =
      raw.captured_at ??
      raw.uploaded_at ??
      raw.date ??
      raw.created_at ??
      null;
    return { label: String(label).trim(), captured_at };
  }
  return null;
}

function dedupeInSystemRows(rows) {
  const seen = new Set();
  const out = [];
  for (const r of rows) {
    const n = normalizeInSystemEntry(r);
    if (!n || !n.label) continue;
    const k = normalizeDocLabelForMatch(n.label);
    if (!k || seen.has(k)) continue;
    seen.add(k);
    out.push(n);
  }
  return out;
}

/** FileWisely `uce-ro-status` often sends the checklist as `checklist` (not `items`). Dedupe by label; earlier wins. */
function dedupeRoItemsByLabelKeepFirst(rows) {
  const seen = new Set();
  const out = [];
  for (const it of rows) {
    if (!it || !it.label) continue;
    const k = normalizeDocLabelForMatch(it.label);
    if (!k || seen.has(k)) continue;
    seen.add(k);
    out.push(it);
  }
  return out;
}

/** Supabase / gateways sometimes wrap the payload in `data` or `result`. */
function unwrapRoStatusBody(raw) {
  if (!raw || typeof raw !== "object") return null;
  const candidates = [raw, raw.data, raw.result, raw.payload, raw.body].filter(
    (x) => x && typeof x === "object" && !Array.isArray(x)
  );
  for (const c of candidates) {
    if (
      c.completeness_level != null ||
      Array.isArray(c.items) ||
      Array.isArray(c.checklist) ||
      c.in_system != null ||
      Array.isArray(c.missing_critical) ||
      Array.isArray(c.documents) ||
      Array.isArray(c.captured_documents)
    ) {
      return c;
    }
  }
  return raw;
}

/** `in_system` may be an array, or a map of doc_key → { label, captured_at, … }. */
function coerceInSystemArray(v) {
  if (v == null) return [];
  if (Array.isArray(v)) return v;
  if (typeof v === "object") {
    const out = [];
    for (const [k, val] of Object.entries(v)) {
      if (val && typeof val === "object" && !Array.isArray(val)) {
        const label =
          val.label ||
          val.name ||
          val.title ||
          val.document_type ||
          val.document_name ||
          k;
        const captured_at =
          val.captured_at ??
          val.uploaded_at ??
          val.date ??
          val.created_at ??
          null;
        out.push({ label: String(label).trim(), captured_at });
      } else if (typeof val === "string") {
        out.push({ label: val.trim(), captured_at: null });
      }
    }
    return out;
  }
  return [];
}

/** Normalize API JSON to a stable shape (unknown fields ignored). */
function normalizeRoStatusPayload(raw) {
  const inner = unwrapRoStatusBody(raw);
  if (!inner || typeof inner !== "object") return null;
  const level = ["green", "yellow", "red"].includes(inner.completeness_level)
    ? inner.completeness_level
    : "green";
  const missing_critical = Array.isArray(inner.missing_critical)
    ? inner.missing_critical.map(String)
    : [];
  const missing_optional = Array.isArray(inner.missing_optional)
    ? inner.missing_optional.map(String)
    : [];
  let hints = [];
  if (Array.isArray(inner.hints)) hints = inner.hints.map(String);
  else if (typeof inner.hints === "string" && inner.hints.trim()) {
    hints = [inner.hints.trim()];
  }
  const fromItems = Array.isArray(inner.items)
    ? inner.items.map(normalizeRoStatusItem).filter(Boolean)
    : [];
  const fromChecklist = Array.isArray(inner.checklist)
    ? inner.checklist.map(normalizeRoStatusItem).filter(Boolean)
    : [];
  const items = dedupeRoItemsByLabelKeepFirst([
    ...fromChecklist,
    ...fromItems,
  ]);
  const recommended_captures = Array.isArray(inner.recommended_captures)
    ? inner.recommended_captures
    : [];
  const inSystemRaw =
    inner.in_system ??
    inner.inSystem ??
    inner.documents_in_system ??
    inner.captured_documents ??
    inner.documents_in_portal ??
    inner.portal_documents ??
    inner.files_in_system ??
    null;
  let fromDocs = [];
  if (Array.isArray(inner.documents) && inner.documents.length > 0) {
    fromDocs = inner.documents.map(normalizeInSystemEntry).filter(Boolean);
  }
  const coerced = coerceInSystemArray(inSystemRaw);
  const fromCapturedItems = items
    .filter((it) => it && it.status === "captured")
    .map((it) => normalizeInSystemEntry(it))
    .filter(Boolean);
  const in_system = dedupeInSystemRows([
    ...coerced.map(normalizeInSystemEntry).filter(Boolean),
    ...fromDocs,
    ...fromCapturedItems,
  ]);
  return {
    completeness_level: level,
    missing_critical,
    missing_optional,
    hints,
    items,
    recommended_captures,
    in_system,
  };
}

/** Panel-only: treat item as in FileWisely (matches server + normalized shapes). */
function roPanelItemIsCaptured(it) {
  if (!it || typeof it !== "object") return false;
  if (it.in_system === true || it.in_filewisely === true || it.captured === true) return true;
  const s = String(it.status ?? "").toLowerCase();
  return (
    s === "captured" ||
    s === "complete" ||
    s === "in_system" ||
    s === "present" ||
    s === "uploaded" ||
    s === "stored" ||
    s === "available" ||
    s === "in_portal" ||
    s === "received" ||
    s === "done" ||
    s === "ingested"
  );
}

function roPanelItemIsSeenNotCaptured(it) {
  if (!it || typeof it !== "object") return false;
  const s = String(it.status ?? "").toLowerCase();
  return s === "seen_not_captured" || s === "seen" || s === "viewed";
}

function roPanelItemIsExplicitMissingStatus(it) {
  if (!it || typeof it !== "object") return false;
  const s = String(it.status ?? "")
    .toLowerCase()
    .replace(/\s+/g, "_")
    .replace(/-/g, "_");
  return s === "missing" || UCE_EXPLICIT_MISSING_STATUSES.has(s);
}

function isDocLabelAccountedBySeenRows(label, seenRows) {
  if (!label || !Array.isArray(seenRows)) return false;
  const n = normalizeDocLabelForMatch(label);
  if (!n) return false;
  for (const r of seenRows) {
    if (!r || !r.label) continue;
    const rn = normalizeDocLabelForMatch(r.label);
    if (!rn) continue;
    if (rn === n) return true;
    if (n.includes(rn) || rn.includes(n)) return true;
  }
  return false;
}

function dedupeDocLabelList(labels) {
  const seen = new Set();
  const out = [];
  for (const raw of labels) {
    const t = String(raw ?? "").trim();
    if (!t) continue;
    const k = normalizeDocLabelForMatch(t);
    if (!k || seen.has(k)) continue;
    seen.add(k);
    out.push(t);
  }
  return out;
}

function clearRoLevelClasses() {
  uceBtn.classList.remove("uce-ro-level-green", "uce-ro-level-yellow", "uce-ro-level-red");
}

function applyRoLevelToButton(level) {
  clearRoLevelClasses();
  if (level === "green") uceBtn.classList.add("uce-ro-level-green");
  else if (level === "yellow") uceBtn.classList.add("uce-ro-level-yellow");
  else if (level === "red") uceBtn.classList.add("uce-ro-level-red");
}

/**
 * @param {boolean} forceRefresh - bypass 30s cache (e.g. 60s background poll).
 */
/**
 * @param {string} [windowTitleForApi] - Title from the window that contained the RO (often CCC); not UCE overlay.
 */
async function fetchRoStatus(roNumber, forceRefresh = false, windowTitleForApi) {
  const url = getRoStatusUrl();
  const bid = getBusinessId();
  if (!url || !bid || !roNumber) {
    roStatusCache = { ro: "", data: null, fetchedAt: 0 };
    return null;
  }

  const now = Date.now();
  if (
    !forceRefresh &&
    roStatusCache.ro === roNumber &&
    roStatusCache.data &&
    now - roStatusCache.fetchedAt < RO_STATUS_CACHE_MS
  ) {
    return roStatusCache.data;
  }

  if (forceRefresh) {
    roStatusCache = { ro: "", data: null, fetchedAt: 0 };
  }

  console.info("📡 [UCE] Fetching RO status", {
    ro: roNumber,
    forceRefresh,
    url,
  });

  const resolvedTitle =
    typeof windowTitleForApi === "string"
      ? windowTitleForApi
      : (lastPolledContext && lastPolledContext.window_title) || "";

  /** Native HTTP bypasses WebView CORS (common cause of "Failed to fetch" to Supabase Edge). */
  let responseText;
  try {
    responseText = await invoke("uce_post_ro_status", {
      url,
      businessId: String(bid).trim(),
      repairOrderNumber: String(roNumber),
      windowTitle: resolvedTitle,
      deviceId: getDeviceId(),
      authorization: SUPABASE_ANON_KEY || "",
    });
  } catch (e) {
    const msg =
      typeof e === "string"
        ? e
        : e?.message || e?.toString?.() || "RO status request failed";
    throw new Error(msg);
  }

  let body;
  try {
    body = JSON.parse(responseText || "{}");
  } catch {
    throw new Error("RO status: response was not valid JSON");
  }
  console.info("📦 [UCE] RO status API response (raw)", roNumber, body);

  const normalized = normalizeRoStatusPayload(body);
  if (normalized) {
    normalized._raw_api = body;
    roStatusCache = { ro: roNumber, data: normalized, fetchedAt: Date.now() };
    console.info("[UCE] RO STATUS (normalized)", {
      ro: roNumber,
      in_system_count: Array.isArray(normalized.in_system)
        ? normalized.in_system.length
        : 0,
      items_count: Array.isArray(normalized.items) ? normalized.items.length : 0,
      level: normalized.completeness_level,
    });
    return normalized;
  }
  console.warn(
    "[UCE] RO status JSON did not normalize (unexpected shape). Check unwrap + in_system/items fields.",
    body
  );
  return {
    _error:
      "RO status API returned JSON this build could not map. See Console for “raw” log; expand below for body.",
    _raw_api: body,
  };
}

async function refreshRoStatusAfterContext() {
  const url = getRoStatusUrl();
  if (!url || !getBusinessId()) {
    clearRoLevelClasses();
    return;
  }
  const { ro, windowTitleForApi } = resolveRoForAwareness();
  if (!ro) {
    clearRoLevelClasses();
    roStatusCache = { ro: "", data: null, fetchedAt: 0 };
    return;
  }

  try {
    const data = await fetchRoStatus(ro, false, windowTitleForApi);
    if (data) {
      applyRoLevelToButton(data.completeness_level);
    } else {
      clearRoLevelClasses();
    }
  } catch (e) {
    console.error("[UCE] RO status:", e);
    clearRoLevelClasses();
  }
}

function buildRoStatusPanel(roNumber, roData) {
  const wrap = document.createElement("div");
  wrap.className = "uce-ro-panel";

  const title = document.createElement("div");
  title.textContent = `RO ${roNumber} — document status`;
  title.style.fontWeight = "700";
  title.style.marginBottom = "6px";
  wrap.appendChild(title);

  if (roData && roData._error) {
    const err = document.createElement("div");
    err.style.color = "#fca5a5";
    err.textContent = roData._error;
    wrap.appendChild(err);
    return wrap;
  }

  if (!roData) {
    const p = document.createElement("div");
    p.style.color = "#94a3b8";
    p.textContent = "No status data (check API URL and tenant).";
    wrap.appendChild(p);
    return wrap;
  }

  const badge = document.createElement("span");
  const lvl = roData.completeness_level || "green";
  badge.className = `uce-ro-badge ${lvl}`;
  badge.textContent =
    lvl === "green" ? "Complete" : lvl === "yellow" ? "Partial" : "Incomplete";
  wrap.appendChild(badge);

  const listLabel = document.createElement("div");
  listLabel.textContent = "Checklist";
  listLabel.style.marginTop = "8px";
  listLabel.style.fontWeight = "600";
  listLabel.style.color = "#cbd5e1";
  wrap.appendChild(listLabel);

  const items = (roData.items && roData.items.length
    ? roData.items
    : []
  ).slice(0, 24);
  if (items.length === 0) {
    const mc = roData.missing_critical || [];
    const mo = roData.missing_optional || [];
    for (const line of mc) {
      const row = document.createElement("div");
      row.className = "uce-ro-row";
      row.textContent = line;
      const chip = document.createElement("span");
      chip.className = "uce-ro-chip crit";
      chip.textContent = "Required — not in FileWisely";
      row.appendChild(chip);
      wrap.appendChild(row);
    }
    for (const line of mo) {
      const row = document.createElement("div");
      row.className = "uce-ro-row";
      row.textContent = line;
      const chip = document.createElement("span");
      chip.className = "uce-ro-chip seen";
      chip.textContent = "Optional — not captured yet";
      row.appendChild(chip);
      wrap.appendChild(row);
    }
    if (mc.length === 0 && mo.length === 0) {
      const row = document.createElement("div");
      row.className = "uce-ro-row";
      row.textContent = "(No per-item breakdown returned)";
      wrap.appendChild(row);
    }
  } else {
    for (const it of items) {
      const row = document.createElement("div");
      row.className = "uce-ro-row";
      const st = (it.status || "").toLowerCase();
      let label = `[${st}] ${it.label}`;
      if (st === "captured") label = `✓ ${it.label}`;
      else if (st === "missing") label = `✗ ${it.label} (confirmed missing)`;
      else if (st === "not_captured_yet") label = `○ ${it.label} (not captured yet)`;
      else if (st === "seen_not_captured") label = `Seen ${it.label}`;
      row.textContent = label;
      if (it.critical) {
        const chip = document.createElement("span");
        chip.className = "uce-ro-chip crit";
        chip.textContent = "Critical";
        row.appendChild(chip);
      }
      if (st === "seen_not_captured") {
        const chip = document.createElement("span");
        chip.className = "uce-ro-chip seen";
        chip.textContent = "Seen";
        row.appendChild(chip);
      }
      wrap.appendChild(row);
    }
  }

  const hints = roData.hints || [];
  for (const h of hints.slice(0, 4)) {
    const hi = document.createElement("div");
    hi.className = "uce-ro-hint";
    hi.textContent = h;
    wrap.appendChild(hi);
  }

  const recs = Array.isArray(roData.recommended_captures)
    ? roData.recommended_captures
    : [];
  if (recs.length > 0) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "uce-ro-action";
    btn.textContent = "Next capture steps (FileWisely)";
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const lines = recs
        .map((r, i) => {
          if (typeof r === "string") return `${i + 1}. ${r}`;
          const lab =
            r.label || r.document_type || r.name || JSON.stringify(r);
          const crit = r.critical ? " (critical)" : "";
          return `${i + 1}. ${lab}${crit}`;
        })
        .join("\n");
      showToast(
        `Recommended captures — RO ${roNumber}:\n${lines}`,
        "warn",
        0
      );
    });
    wrap.appendChild(btn);
  }

  return wrap;
}

function buildRoStatusUnavailablePanel(roUrl, roNum) {
  const wrap = document.createElement("div");
  wrap.className = "uce-ro-panel";
  const h = document.createElement("div");
  h.style.fontWeight = "700";
  h.style.marginBottom = "6px";
  h.textContent = "RO / FileWisely document status";
  wrap.appendChild(h);

  if (!roUrl) {
    const p = document.createElement("div");
    p.style.color = "#94a3b8";
    p.textContent = "—";
    wrap.appendChild(p);
    return wrap;
  }

  if (!roNum) {
    const p = document.createElement("div");
    p.style.color = "#94a3b8";
    p.textContent = "No RO detected";
    wrap.appendChild(p);
    return wrap;
  }

  return wrap;
}

function shortenStaffLabel(text, maxLen = 36) {
  const s = String(text || "").trim();
  if (!s) return "";
  if (s.length <= maxLen) return s;
  return `${s.slice(0, Math.max(0, maxLen - 1))}…`;
}

/** Highest-priority “missing next” for the compact panel. */
function pickNextMissingForStaffPanel(
  confirmedMissingDedup,
  notCapturedYetDedup,
  roData
) {
  if (confirmedMissingDedup.length > 0) {
    return { label: confirmedMissingDedup[0], kind: "confirmed" };
  }
  const recs = Array.isArray(roData?.recommended_captures)
    ? roData.recommended_captures
    : [];
  for (const r of recs) {
    const lab =
      typeof r === "string" ? r : r?.label || r?.document_type || r?.name || "";
    const t = String(lab).trim();
    if (t) return { label: t, kind: "suggested" };
  }
  if (notCapturedYetDedup.length > 0) {
    return { label: notCapturedYetDedup[0], kind: "maybe" };
  }
  return null;
}

function uceProdLongDocHintText() {
  return "In System comes from FileWisely (uce-ro-status). Confirmed missing = API required-but-absent or explicit missing status. “Seen in CCC” is when UCE matched CCC window titles (estimate, supplement, final bill, print, etc.).";
}

function appendProdLabelList(parent, labels, listClass, maxItems = 20) {
  if (!labels.length) {
    const em = document.createElement("p");
    em.className = "uce-prod-mini";
    em.textContent = "None";
    parent.appendChild(em);
    return;
  }
  const ul = document.createElement("ul");
  ul.className = listClass;
  for (const lab of labels.slice(0, maxItems)) {
    const li = document.createElement("li");
    li.textContent = shortenStaffLabel(lab);
    ul.appendChild(li);
  }
  parent.appendChild(ul);
}

/** Staff-facing RO panel — compact checklist; details under “More”. */
function buildProductionRoPanel(roUrl, tid, roNum, roData) {
  const root = document.createElement("div");
  root.className = "uce-prod-shell uce-prod-shell--compact";

  function appendMoreBlock(title, bodyFn) {
    const block = document.createElement("div");
    block.className = "uce-prod-detail-block";
    const t = document.createElement("div");
    t.className = "uce-prod-block-title";
    t.textContent = title;
    block.appendChild(t);
    bodyFn(block);
    return block;
  }

  function appendProdMoreSection(
    inSystemRows,
    seenRows,
    notCapturedYetDedup,
    confirmedMissingDedup,
    complete,
    trustPct,
    roPayload
  ) {
    const details = document.createElement("details");
    details.className = "uce-prod-more-details";
    const sum = document.createElement("summary");
    sum.textContent = "More";
    details.appendChild(sum);

    if (seenRows.length > 0) {
      details.appendChild(
        appendMoreBlock("Seen in CCC", (block) => {
          const ul = document.createElement("ul");
          ul.className = "uce-prod-checklist";
          for (const row of seenRows.slice(0, 24)) {
            const li = document.createElement("li");
            li.appendChild(document.createTextNode(shortenStaffLabel(row.label)));
            if (row.sub) {
              const sm = document.createElement("span");
              sm.className = "uce-prod-li-sub";
              sm.textContent = row.sub;
              li.appendChild(sm);
            }
            ul.appendChild(li);
          }
          block.appendChild(ul);
        })
      );
    }

    details.appendChild(
      appendMoreBlock("In system (with times)", (block) => {
        if (!inSystemRows.length) {
          const p = document.createElement("p");
          p.className = "uce-prod-mini";
          p.textContent = "None";
          block.appendChild(p);
          return;
        }
        const ul = document.createElement("ul");
        ul.className = "uce-prod-checklist";
        for (const row of inSystemRows.slice(0, 24)) {
          const li = document.createElement("li");
          const dt = formatCapturedAtDisplay(row.captured_at);
          li.textContent = dt
            ? `${shortenStaffLabel(row.label)} — ${dt}`
            : shortenStaffLabel(row.label);
          ul.appendChild(li);
        }
        block.appendChild(ul);
      })
    );

    details.appendChild(
      appendMoreBlock("Not captured yet", (block) => {
        appendProdLabelList(block, notCapturedYetDedup, "uce-prod-checklist");
      })
    );

    details.appendChild(
      appendMoreBlock("Missing (confirmed), all", (block) => {
        appendProdLabelList(block, confirmedMissingDedup, "uce-prod-checklist");
      })
    );

    details.appendChild(
      appendMoreBlock("FileWisely status", (block) => {
        const p = document.createElement("p");
        p.className = "uce-prod-mini";
        p.textContent = complete ? "Complete" : "Incomplete";
        block.appendChild(p);
      })
    );

    if (trustPct != null) {
      details.appendChild(
        appendMoreBlock("UCE reliability (local)", (block) => {
          const p = document.createElement("p");
          p.className = "uce-prod-mini";
          p.textContent = `${trustPct}% — captures vs. context signals on this PC.`;
          block.appendChild(p);
        })
      );
    }

    const recs = Array.isArray(roPayload?.recommended_captures)
      ? roPayload.recommended_captures
      : [];
    if (recs.length > 0) {
      details.appendChild(
        appendMoreBlock("Recommended sequence", (block) => {
          const lines = recs
            .map((r, i) => {
              if (typeof r === "string") return `${i + 1}. ${r}`;
              const lab = r.label || r.document_type || r.name || "Item";
              return `${i + 1}. ${lab}`;
            })
            .join("\n");
          const p = document.createElement("pre");
          p.className = "uce-prod-raw-json";
          p.style.maxHeight = "120px";
          p.textContent = lines;
          block.appendChild(p);
        })
      );
    }

    details.appendChild(
      appendMoreBlock("How this panel is built", (block) => {
        const p = document.createElement("p");
        p.className = "uce-prod-mini";
        p.textContent = uceProdLongDocHintText();
        block.appendChild(p);
      })
    );

    if (roPayload?._raw_api && isUceRoDebugRawEnabled()) {
      const wrap = document.createElement("div");
      wrap.className = "uce-prod-raw-wrap";
      const pre = document.createElement("pre");
      pre.className = "uce-prod-raw-json";
      pre.textContent = JSON.stringify(roPayload._raw_api, null, 2);
      wrap.appendChild(pre);
      details.appendChild(
        appendMoreBlock("Raw API (debug)", (block) => {
          block.appendChild(wrap);
        })
      );
    }

    root.appendChild(details);
  }

  const top = document.createElement("div");
  top.className = "uce-prod-top";
  const roLine = document.createElement("div");
  roLine.className = "uce-prod-ro-line";
  const roNumEl = document.createElement("div");
  roNumEl.className = "uce-prod-ro-num";
  roNumEl.textContent = roNum ? `RO ${roNum}` : "No RO in title";
  roLine.appendChild(roNumEl);
  top.appendChild(roLine);
  root.appendChild(top);

  if (!roNum) {
    const hint = document.createElement("p");
    hint.className = "uce-prod-mini";
    hint.textContent = "Open a repair order in CCC to see FileWisely status.";
    root.appendChild(hint);
    return root;
  }

  if (!tid || !roUrl) {
    const p = document.createElement("p");
    p.className = "uce-prod-mini";
    p.textContent = !tid
      ? "Add your FileWisely business ID in settings to load documents."
      : "RO status URL is not configured for this build.";
    root.appendChild(p);
    const details = document.createElement("details");
    details.className = "uce-prod-more-details";
    const sumCfg = document.createElement("summary");
    sumCfg.textContent = "More";
    details.appendChild(sumCfg);
    const hint2 = document.createElement("p");
    hint2.className = "uce-prod-mini";
    hint2.textContent = uceProdLongDocHintText();
    details.appendChild(hint2);
    root.appendChild(details);
    return root;
  }

  if (roData && roData._error) {
    const p = document.createElement("p");
    p.className = "uce-prod-mini";
    p.style.color = "#fecaca";
    p.textContent = String(roData._error);
    root.appendChild(p);
    const details = document.createElement("details");
    details.className = "uce-prod-more-details";
    const sumErr = document.createElement("summary");
    sumErr.textContent = "More";
    details.appendChild(sumErr);
    if (roData._raw_api) {
      const pre = document.createElement("pre");
      pre.className = "uce-prod-raw-json";
      pre.textContent = JSON.stringify(roData._raw_api, null, 2);
      details.appendChild(pre);
    }
    const hint2 = document.createElement("p");
    hint2.className = "uce-prod-mini";
    hint2.textContent = uceProdLongDocHintText();
    details.appendChild(hint2);
    root.appendChild(details);
    return root;
  }

  if (!roData) {
    const p = document.createElement("p");
    p.className = "uce-prod-mini";
    p.textContent = "Could not load status. Check network and RO URL.";
    root.appendChild(p);
    const details = document.createElement("details");
    details.className = "uce-prod-more-details";
    const sumEmpty = document.createElement("summary");
    sumEmpty.textContent = "More";
    details.appendChild(sumEmpty);
    const hint2 = document.createElement("p");
    hint2.className = "uce-prod-mini";
    hint2.textContent = uceProdLongDocHintText();
    details.appendChild(hint2);
    root.appendChild(details);
    return root;
  }

  const complete = roData.completeness_level === "green";
  const trustPct = getSystemConfidencePercentForRo(roNum);

  let inSystemRows = [];
  const inRaw = Array.isArray(roData.in_system)
    ? roData.in_system
    : Array.isArray(roData.inSystem)
      ? roData.inSystem
      : [];
  for (const e of inRaw) {
    const n = normalizeInSystemEntry(e);
    if (n) inSystemRows.push(n);
  }
  if (inSystemRows.length === 0 && Array.isArray(roData.items)) {
    for (const it of roData.items) {
      if (typeof it !== "object" || !it) continue;
      if (!roPanelItemIsCaptured(it)) continue;
      const lab = it.label || it.name || it.title;
      if (!lab) continue;
      inSystemRows.push({
        label: String(lab),
        captured_at: it.captured_at ?? it.uploaded_at ?? null,
      });
    }
  }
  inSystemRows = dedupeInSystemRows(inSystemRows);

  const seenRows = [];
  const localSeen = loadCccSeenForRo(roNum);

  for (const key of Object.keys(localSeen)) {
    const rec = localSeen[key];
    if (!rec || !rec.label) continue;
    if (isDocLabelInInSystemList(rec.label, inSystemRows)) continue;
    const sub = rec.firstSeenAt
      ? `first seen ${formatCapturedAtDisplay(rec.firstSeenAt)}`
      : null;
    seenRows.push({ label: rec.label, sub });
  }
  if (Array.isArray(roData.items)) {
    for (const it of roData.items) {
      if (typeof it !== "object" || !it) continue;
      if (!roPanelItemIsSeenNotCaptured(it)) continue;
      const lab = it.label || it.name || it.title;
      if (!lab) continue;
      if (isDocLabelInInSystemList(lab, inSystemRows)) continue;
      const nd = normalizeDocLabelForMatch(lab);
      if (seenRows.some((r) => normalizeDocLabelForMatch(r.label) === nd)) continue;
      seenRows.push({
        label: lab,
        sub: null,
      });
    }
  }

  const explicitMissingFromItems = [];
  const ambiguousChecklistLabels = [];
  if (Array.isArray(roData.items) && roData.items.length > 0) {
    for (const it of roData.items) {
      if (typeof it === "string") {
        ambiguousChecklistLabels.push(String(it).trim());
        continue;
      }
      const lab = it.label || it.name || it.title || "";
      if (!lab) continue;
      if (roPanelItemIsCaptured(it) || roPanelItemIsSeenNotCaptured(it)) continue;
      if (roPanelItemIsExplicitMissingStatus(it)) {
        explicitMissingFromItems.push(String(lab).trim());
      } else {
        ambiguousChecklistLabels.push(String(lab).trim());
      }
    }
  }

  try {
    window.__uceLastRoSupplementTruthModel = buildRoSupplementTruthModel({
      inSystemLabels: inSystemRows.map((r) => r.label),
      missingCriticalLabels: (roData.missing_critical || []).map((x) =>
        String(x).trim()
      ),
      explicitChecklistMissingLabels: explicitMissingFromItems,
      localSeenByKey: localSeen,
    });
  } catch (e) {
    console.warn("[UCE] buildRoSupplementTruthModel:", e);
    window.__uceLastRoSupplementTruthModel = null;
  }

  const confirmedMissingCandidates = [
    ...(roData.missing_critical || []).map((x) => String(x).trim()),
    ...explicitMissingFromItems,
  ];
  const notCapturedYetCandidates = [
    ...(roData.missing_optional || []).map((x) => String(x).trim()),
    ...ambiguousChecklistLabels,
  ];

  const confirmedMissingDedup = dedupeDocLabelList(
    confirmedMissingCandidates.filter(
      (lab) =>
        !isDocLabelInInSystemList(lab, inSystemRows) &&
        !isDocLabelAccountedBySeenRows(lab, seenRows)
    )
  );
  const confirmedKeys = new Set(
    confirmedMissingDedup.map((l) => normalizeDocLabelForMatch(l)).filter(Boolean)
  );
  const notCapturedYetDedup = dedupeDocLabelList(
    notCapturedYetCandidates.filter(
      (lab) =>
        !isDocLabelInInSystemList(lab, inSystemRows) &&
        !isDocLabelAccountedBySeenRows(lab, seenRows) &&
        !confirmedKeys.has(normalizeDocLabelForMatch(lab))
    )
  );

  const inCount = inSystemRows.length;
  const missCount = confirmedMissingDedup.length;

  const chipRow = document.createElement("div");
  chipRow.className = "uce-prod-chip-row";
  const chipIn = document.createElement("span");
  chipIn.className = "uce-prod-chip";
  chipIn.textContent = `${inCount} in system`;
  chipRow.appendChild(chipIn);
  if (missCount > 0) {
    const chipMiss = document.createElement("span");
    chipMiss.className = "uce-prod-chip uce-prod-chip--bad";
    chipMiss.textContent =
      missCount === 1 ? "1 missing" : `${missCount} missing`;
    chipRow.appendChild(chipMiss);
  } else if (complete) {
    const chipOk = document.createElement("span");
    chipOk.className = "uce-prod-chip uce-prod-chip--ok";
    chipOk.textContent = "Complete";
    chipRow.appendChild(chipOk);
  }
  top.appendChild(chipRow);

  const secIn = document.createElement("div");
  secIn.className = "uce-prod-block";
  const hIn = document.createElement("div");
  hIn.className = "uce-prod-block-title";
  hIn.textContent = "In system";
  secIn.appendChild(hIn);
  appendProdLabelList(
    secIn,
    inSystemRows.map((r) => r.label),
    "uce-prod-checklist"
  );
  root.appendChild(secIn);

  const nextMissing = pickNextMissingForStaffPanel(
    confirmedMissingDedup,
    notCapturedYetDedup,
    roData
  );
  const secMiss = document.createElement("div");
  secMiss.className = "uce-prod-block";
  const hMiss = document.createElement("div");
  hMiss.className = "uce-prod-block-title";
  hMiss.textContent = "Missing next";
  secMiss.appendChild(hMiss);
  const nextEl = document.createElement("p");
  nextEl.className = "uce-prod-next";
  if (nextMissing) {
    nextEl.textContent = shortenStaffLabel(nextMissing.label, 42);
    if (nextMissing.kind === "maybe") {
      nextEl.classList.add("uce-prod-next--maybe");
    }
  } else {
    nextEl.classList.add("uce-prod-next--none");
    nextEl.textContent = "None flagged";
  }
  secMiss.appendChild(nextEl);

  const doNow = document.createElement("p");
  doNow.className = "uce-prod-do";
  if (nextMissing?.kind === "confirmed") {
    doNow.textContent =
      "Re-print to FileWisely Incoming, or use the blue camera button when that document is on screen.";
  } else if (nextMissing?.kind === "suggested") {
    doNow.textContent =
      "When it’s on screen, use the blue camera button or print to Incoming.";
  } else if (nextMissing?.kind === "maybe") {
    doNow.textContent = "When CCC shows it, use the blue camera button.";
  } else if (complete) {
    doNow.textContent = "No urgent doc — watch CCC for the next step.";
  } else {
    doNow.textContent =
      "Use the blue camera button when the next document is ready.";
  }
  secMiss.appendChild(doNow);
  root.appendChild(secMiss);

  appendProdMoreSection(
    inSystemRows,
    seenRows,
    notCapturedYetDedup,
    confirmedMissingDedup,
    complete,
    trustPct,
    roData
  );

  return root;
}

/** Staff-facing RO / FileWisely panel — on demand only. */
async function showProductionRoPanel() {
  const tid = getBusinessId();
  const roResolved = resolveRoForAwareness();
  const roUrl = getRoStatusUrl();
  const roNum = roResolved.ro;
  console.info("📡 [UCE] RO panel open — force refresh", {
    ro: roNum,
    hasUrl: !!roUrl,
    hasTenant: !!tid,
  });
  let roPayload = null;
  if (roUrl && tid && roNum) {
    try {
      roPayload = await fetchRoStatus(roNum, true, roResolved.windowTitleForApi);
    } catch (e) {
      roPayload = {
        _error:
          typeof e === "string"
            ? e
            : e?.message || e?.toString?.() || "RO status request failed",
      };
    }
  }

  debugSheetEl.replaceChildren();
  debugSheetEl.classList.add("uce-debug-sheet--prod");
  debugSheetEl.appendChild(buildProductionRoPanel(roUrl, tid, roNum, roPayload));
  await pickRightPeekSide();
  appEl.classList.add("uce-debug-open");
  debugSheetEl.hidden = false;
  debugSheetEl.setAttribute("aria-hidden", "false");
  await fitRightPeekLayout();
  peekSidePanelMode = "production";
}

function dismissToast() {
  if (toastTimer) {
    clearTimeout(toastTimer);
    toastTimer = null;
  }
  if (!toastEl.className.includes("show")) return;
  toastEl.className = "toast";
  setToastLayoutOpen(false);
  void setCompactWindowSize();
}

/** Reserved for future (e.g. RTL); column layout keeps toolbar above the sheet. */
async function pickRightPeekSide() {
  /* no-op */
}

/**
 * Toolbar on top, RO/debug sheet below. Window **top-left never moves** — we only grow/shrink size
 * and cap dimensions to the monitor work area from that anchor so the sheet scrolls instead of
 * repositioning the pill (clamp would shift Y and feel like the button “falls” down the screen).
 */
async function fitRightPeekLayout() {
  const posBefore = await appWindow.outerPosition();
  void debugSheetEl.offsetHeight;
  const r = debugSheetEl.getBoundingClientRect();
  const contentH = Math.max(
    r.height,
    debugSheetEl.scrollHeight || 0
  );
  const gap = 8;
  const innerPad = 12;
  /** Matches `#app.uce-debug-open` vertical padding (8px + 10px). */
  const appChromeY = 18;
  const compact = getCompactWindowSize();
  const toolbarH = compact.height;
  const isProdSheet = debugSheetEl.classList.contains("uce-debug-sheet--prod");
  const sheetW = isProdSheet ? 300 : 340;
  const maxInner = isProdSheet ? 340 : 320;
  const innerH = Math.max(
    36,
    Math.min(maxInner, Math.ceil(contentH) || 120)
  );
  let w = Math.ceil(
    Math.min(700, Math.max(compact.width + innerPad * 2, sheetW + innerPad * 2))
  );
  let h = Math.ceil(
    Math.min(
      680,
      appChromeY + innerPad + toolbarH + gap + innerH + innerPad
    )
  );

  try {
    const mon = await resolveMonitorForClamp();
    if (mon) {
      const wa = mon.workArea;
      const margin = 10;
      const maxRight = wa.position.x + wa.size.width - margin;
      const maxBottom = wa.position.y + wa.size.height - margin;
      const maxW = Math.max(120, maxRight - posBefore.x);
      const maxH = Math.max(120, maxBottom - posBefore.y);
      w = Math.min(w, maxW);
      h = Math.min(h, maxH);
    }
  } catch (_) {
    /* ignore */
  }

  try {
    await invoke("uce_set_overlay_logical_size", { width: w, height: h });
    await delayToastLayout(90);
    await appWindow.setPosition(
      new PhysicalPosition(posBefore.x, posBefore.y)
    );
    await delayToastLayout(50);
  } catch (e) {
    console.error("fitRightPeekLayout:", e);
  }
}

async function dismissRightPeek() {
  if (!appEl.classList.contains("uce-debug-open")) return;
  rightPeekActive = false;
  peekSidePanelMode = null;
  debugSheetEl.hidden = true;
  debugSheetEl.replaceChildren();
  debugSheetEl.classList.remove("uce-debug-sheet--prod");
  debugSheetEl.setAttribute("aria-hidden", "true");
  appEl.classList.remove("uce-debug-open");
  const posBefore = await appWindow.outerPosition();
  try {
    await setCompactWindowSize();
    await appWindow.setPosition(new PhysicalPosition(posBefore.x, posBefore.y));
  } catch (e) {
    console.error("dismissRightPeek:", e);
  }
}

/** Shown at the top of in-overlay success/info toasts (not the Windows printer driver balloon). */
const TOAST_BRAND = "UCE";

/** `durationMs <= 0` = stay open until {@link dismissToast} (e.g. Esc). */
function showToast(text, kind = "success", durationMs = 3600) {
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = null;
  toastEl.replaceChildren();
  const brand = document.createElement("div");
  brand.className = "uce-toast-brand";
  brand.textContent = TOAST_BRAND;
  const msg = document.createElement("div");
  msg.className = "uce-toast-msg";
  msg.textContent = text;
  toastEl.appendChild(brand);
  toastEl.appendChild(msg);
  toastEl.className = `toast ${kind} show`;
  setToastLayoutOpen(true);

  const scheduleFit = () => {
    void fitWindowToToast();
  };
  // One pass after layout, one after fonts/multi-line height settle — avoid 6+ native resizes (was flashing the whole overlay).
  requestAnimationFrame(() => {
    requestAnimationFrame(scheduleFit);
  });
  setTimeout(scheduleFit, 180);

  if (durationMs > 0) {
    toastTimer = setTimeout(() => dismissToast(), durationMs);
  }
}

function triggerFlash() {
  flashEl.classList.remove("on");
  void flashEl.offsetWidth;
  flashEl.classList.add("on");
}

/**
 * Doc family when the desktop can determine it reliably.
 * Returns: "estimate" | "supplement" | "final_bill" | null
 * null => screenshot uses quick_screenshot so backend AI classifies.
 */
function resolveDocumentSubtype(matchedRule, windowTitle) {
  const rule = (matchedRule || "").toLowerCase();
  const title = (windowTitle || "").toLowerCase();

  if (rule === "ccc_supplement") return "supplement";
  if (rule === "ccc_final_bill") return "final_bill";
  if (rule === "ccc_estimate") return "estimate";

  if (rule === "ccc_open" || rule.startsWith("ccc_trained_") || rule.startsWith("ccc_")) {
    if (title.includes("final bill")) return "final_bill";
    if (title.includes("supplement") || /\bsupp\b/.test(title)) return "supplement";
    if (title.includes("estimate")) return "estimate";
    return null;
  }

  if (title.includes("final bill")) return "final_bill";
  if (title.includes("supplement") || /\bsupp\b/.test(title)) return "supplement";
  if (title.includes("estimate")) return "estimate";
  return null;
}

/**
 * document_type for ingest.
 * Screenshots: estimate_screenshot | supplement_screenshot | final_bill_screenshot when
 * subtype is known; else quick_screenshot (backend AI classifies).
 * PDFs: parallel *_pdf names; unknown subtype defaults to estimate_pdf for folder exports.
 */
function resolveDocumentType(matchedRule, isPdf, windowTitle) {
  const sub = resolveDocumentSubtype(matchedRule, windowTitle);
  if (isPdf) {
    if (sub === "supplement") return "supplement_pdf";
    if (sub === "final_bill") return "final_bill_pdf";
    if (sub === "estimate") return "estimate_pdf";
    return "estimate_pdf";
  }
  if (sub === "supplement") return "supplement_screenshot";
  if (sub === "final_bill") return "final_bill_screenshot";
  if (sub === "estimate") return "estimate_screenshot";
  return "quick_screenshot";
}

function inferPreferredModeFromContext(ctx) {
  if (!ctx || typeof ctx !== "object") return "screenshot";
  const wk = (ctx.workflow_kind || "").toLowerCase();
  if (wk === "ccc") return "pdf";
  if (
    wk &&
    wk !== "unknown" &&
    wk !== "excluded" &&
    wk !== "ccc"
  ) {
    return "screenshot";
  }
  const direct = (ctx.preferred_capture_mode || "").toLowerCase();
  if (direct === "pdf" || direct === "screenshot") return direct;
  const rule = (ctx?.matched_rule || "").toLowerCase();
  const title = (ctx?.window_title || "").toLowerCase();
  const isPdfRule =
    rule.startsWith("ccc_") || rule.startsWith("ccc_trained");
  const isCccTitle = title.includes("ccc") || title.includes("ccc one");
  const isLeadingRoTitle = /^\d{4,6}\b/.test(title.trim());
  const isRoTitle =
    title.includes("repair order") ||
    title.includes("ro ") ||
    title.includes("ro#") ||
    title.includes("ro-");
  const isRoNumeric = /\bro[#:-]?\d{3,}\b/.test(title);
  const isFilewiselyApp =
    title.includes("filewisely") || title.includes("file wisely");
  return isPdfRule ||
    isCccTitle ||
    isLeadingRoTitle ||
    isRoTitle ||
    isRoNumeric ||
    isFilewiselyApp
    ? "pdf"
    : "screenshot";
}

function getStableContextForModeDecision() {
  const now = Date.now();
  if (inferPreferredModeFromContext(lastPolledContext) === "pdf") {
    return lastPolledContext || {};
  }
  if (lastKnownContext && now - lastKnownContextAt <= 20000) {
    return lastKnownContext;
  }
  return lastPolledContext || {};
}

function resolvePreferredMode(contextSnapshot) {
  const directMode =
    contextSnapshot?.preferred_capture_mode ||
    inferPreferredModeFromContext(contextSnapshot);
  if (directMode === "pdf") return "pdf";

  const now = Date.now();
  const recentKnownPdf =
    lastKnownContext &&
    inferPreferredModeFromContext(lastKnownContext) === "pdf" &&
    now - lastKnownContextAt <= 120000;
  return recentKnownPdf ? "pdf" : "screenshot";
}

async function saveCurrentPosition() {
  const pos = await appWindow.outerPosition();
  await invoke("save_window_position", { x: pos.x, y: pos.y });
}

let lastPointerClientX = 0;
let lastPointerClientY = 0;

/** Idle dock ≈68% opacity; full when pointer is over the dock, while pressed/dragging, or when a modal/toast needs clarity. */
function updateDockHoverOpacity(clientX, clientY) {
  if (!uceDockEl) return;
  if (dockNativeDragStarted || pointerIsDown) {
    uceDockEl.classList.add("uce-dock--opaque");
    uceDockEl.classList.remove("uce-dock--hot");
    return;
  }
  if (
    appEl.classList.contains("uce-debug-open") ||
    appEl.classList.contains("uce-tenant-setup-open") ||
    appEl.classList.contains("uce-printer-severe-open") ||
    toastEl.className.includes("show")
  ) {
    uceDockEl.classList.add("uce-dock--opaque");
    uceDockEl.classList.remove("uce-dock--hot");
    return;
  }
  uceDockEl.classList.remove("uce-dock--opaque");
  const r = uceDockEl.getBoundingClientRect();
  if (r.width <= 0 || r.height <= 0) {
    uceDockEl.classList.remove("uce-dock--hot");
    return;
  }
  const inside =
    clientX >= r.left &&
    clientX <= r.right &&
    clientY >= r.top &&
    clientY <= r.bottom;
  uceDockEl.classList.toggle("uce-dock--hot", inside);
}

function pulseCaptureCueOnce() {
  if (!uceBtn) return;
  uceBtn.classList.remove("uce-btn--cue-pulse");
  void uceBtn.offsetWidth;
  uceBtn.classList.add("uce-btn--cue-pulse");
  window.setTimeout(() => uceBtn.classList.remove("uce-btn--cue-pulse"), 950);
}

/** Min interval between `uce-context-detected` emits for the same `type|roId` (reduces Theo noise). */
const THEO_CONTEXT_EMIT_MIN_INTERVAL_MS = 10_000;
/** @type {Map<string, number>} */
const theoContextEmitLastAt = new Map();

/**
 * If FileWisely already lists this supplement for the RO, skip Theo “missing supplement” style prompts.
 */
function shouldSuppressTheoSupplementWhenInFileWisely(stableDetected, roId, windowTitle) {
  if (stableDetected?.type !== "ccc_supplement") return false;
  if (!roId) return false;
  if (roStatusCache.ro !== roId || !roStatusCache.data) return false;
  const data = roStatusCache.data;
  if (data._error) return false;
  const inSystem = Array.isArray(data.in_system) ? data.in_system : [];
  const sigs = inferCccDocSignalsFromTitle(String(windowTitle || "").trim());
  const supp = sigs.find((s) => String(s.key).startsWith("supplement_"));
  if (!supp) return false;
  return isDocLabelInInSystemList(supp.label, inSystem);
}

function maybeEmitUceContextForTheo(stableDetected, roId, windowTitle, reasonPayload) {
  if (!stableDetected || stableDetected.bucket !== "known") return;
  const interesting = new Set([
    "ccc_supplement",
    "ccc_estimate",
    "ccc_final_bill",
    "ccc_print_dialog",
    "tesla_epc",
    "parts_invoice",
  ]);
  if (!interesting.has(stableDetected.type)) return;
  if (shouldSuppressTheoSupplementWhenInFileWisely(stableDetected, roId, windowTitle)) {
    return;
  }
  const key = `${stableDetected.type}|${roId || ""}`;
  const now = Date.now();
  const lastAt = theoContextEmitLastAt.get(key);
  if (lastAt != null && now - lastAt < THEO_CONTEXT_EMIT_MIN_INTERVAL_MS) {
    return;
  }
  theoContextEmitLastAt.set(key, now);
  armPendingMissingSuppressBuffer(now);
  if (roId) noteUceContextDetectionEmitted(roId);
  const suppressUntil = getSuppressAggressiveMissingUntilMs();
  emit("uce-context-detected", {
    type: stableDetected.type,
    roId: roId || null,
    timestamp: new Date().toISOString(),
    bucket: stableDetected.bucket,
    confidence: stableDetected.confidence,
    preferredCaptureMode: stableDetected.preferredCaptureMode,
    decisionReasons: reasonPayload?.decisionReasons?.length
      ? [...reasonPayload.decisionReasons]
      : [],
    suggestedTheoTone: reasonPayload?.suggestedTheoTone ?? "neutral",
    reasonHeadline: reasonPayload?.headline ?? null,
    reasonBullets: reasonPayload?.bullets?.length
      ? [...reasonPayload.bullets]
      : [],
    suppressAggressiveMissingUntil: new Date(suppressUntil).toISOString(),
    pendingCaptureSuppressMs: getPendingCaptureSuppressMs(),
    messagingHint:
      stableDetected.type === "ccc_supplement"
        ? "Prefer: “Supplement visible in CCC — waiting for capture…” over blunt “missing” while buffer active."
        : "Context visible in UCE — use waiting-for-capture tone if doc not yet in FileWisely.",
  }).catch((e) => console.warn("[UCE] uce-context-detected emit failed:", e));

  const tsIso = new Date().toISOString();
  if (stableDetected.type === "ccc_supplement") {
    const sigs = inferCccDocSignalsFromTitle(String(windowTitle || "").trim());
    const sn = sigs.find((s) => String(s.key).startsWith("supplement_"));
    const n = sn
      ? Number(String(sn.key).replace(/^supplement_/, ""))
      : NaN;
    emit("uce-seen-in-ccc", {
      ro: roId || null,
      seen_in_ccc: true,
      related_type: "supplement",
      ...(Number.isFinite(n) ? { supplement_number: n } : {}),
      timestamp: tsIso,
    }).catch((e) => console.warn("[UCE] uce-seen-in-ccc emit failed:", e));
  } else if (stableDetected.type === "ccc_print_dialog") {
    emit("uce-seen-in-ccc", {
      ro: roId || null,
      seen_in_ccc: true,
      related_type: "print_panel",
      timestamp: tsIso,
    }).catch((e) => console.warn("[UCE] uce-seen-in-ccc emit failed:", e));
  }
}

/** After native drag: snap overlay window to the nearest work-area corner (light settle animation on the toolbar). */
async function snapWindowToNearestCornerZone() {
  const mon = await resolveMonitorForClamp();
  if (!mon) return;
  const pos = await appWindow.outerPosition();
  const size = await appWindow.outerSize();
  const wa = mon.workArea;
  const margin = 10;
  const w = size.width;
  const h = size.height;
  const cx = pos.x + w / 2;
  const cy = pos.y + h / 2;
  const minX = wa.position.x + margin;
  const minY = wa.position.y + margin;
  const maxX = wa.position.x + wa.size.width - w - margin;
  const maxY = wa.position.y + wa.size.height - h - margin;
  if (maxX < minX || maxY < minY) {
    const x = Math.min(
      Math.max(pos.x, wa.position.x + margin),
      wa.position.x + wa.size.width - w - margin
    );
    const y = Math.min(
      Math.max(pos.y, wa.position.y + margin),
      wa.position.y + wa.size.height - h - margin
    );
    if (x !== pos.x || y !== pos.y) {
      try {
        await appWindow.setPosition(new PhysicalPosition(x, y));
      } catch (e) {
        console.error("snapWindowToNearestCornerZone:", e);
      }
    }
    return;
  }
  const corners = [
    { x: minX, y: minY },
    { x: maxX, y: minY },
    { x: maxX, y: maxY },
    { x: minX, y: maxY },
  ];
  let best = corners[0];
  let bestD = Infinity;
  for (const c of corners) {
    const tcx = c.x + w / 2;
    const tcy = c.y + h / 2;
    const d = Math.hypot(cx - tcx, cy - tcy);
    if (d < bestD) {
      bestD = d;
      best = c;
    }
  }
  if (best.x !== pos.x || best.y !== pos.y) {
    try {
      await appWindow.setPosition(new PhysicalPosition(best.x, best.y));
      if (uceDockEl) {
        uceDockEl.classList.remove("uce-dock--snap-bump");
        void uceDockEl.offsetWidth;
        uceDockEl.classList.add("uce-dock--snap-bump");
        window.setTimeout(() => uceDockEl.classList.remove("uce-dock--snap-bump"), 280);
      }
    } catch (e) {
      console.error("snapWindowToNearestCornerZone:", e);
    }
  }
}

function truncateFwPipelineDetail(s, max = 8000) {
  if (s == null) return "";
  const str = typeof s === "string" ? s : String(s);
  return str.length > max ? `${str.slice(0, max)}…` : str;
}

function fwTruthyId(v) {
  if (v == null) return false;
  const s = String(v).trim();
  return s.length > 0;
}

function hasMatchedPortalRecord(body) {
  if (!body || typeof body !== "object") return false;
  const mr = body.matched_record;
  if (mr != null && mr !== false) {
    if (typeof mr === "object" && !Array.isArray(mr) && Object.keys(mr).length === 0)
      return false;
    return true;
  }
  if (body.portal_record != null && body.portal_record !== false) return true;
  if (body.repair_order_match === true || body.ro_matched === true) return true;
  if (body.document_matched === true || body.matched_to_portal === true) return true;
  const ms = String(body.match_status || body.document_match_status || "").toLowerCase();
  if (ms === "matched" || ms === "linked") return true;
  if (fwTruthyId(body.linked_ro_id) || fwTruthyId(body.linked_repair_order_id)) return true;
  if (fwTruthyId(body.portal_document_id) || fwTruthyId(body.filewisely_document_id))
    return true;
  return false;
}

/** `success === true` alone is not enough — require an explicit persistence signal from the backend. */
function hasExplicitDurableSuccess(body) {
  if (!body || typeof body !== "object" || body.success !== true) return false;
  if (body.persisted === true) return true;
  if (body.durable_persistence === true) return true;
  if (body.persistence?.confirmed === true || body.persistence?.persisted === true) return true;
  if (body.ingest?.persisted === true || body.ingestion?.persisted === true) return true;
  if (body.storage?.persisted === true) return true;
  const st = String(body.ingestion_status || body.storage_status || "").toLowerCase();
  if (st === "persisted" || st === "stored" || st === "committed") return true;
  return false;
}

function getPortalProcessedConfirmation(body) {
  if (!body || typeof body !== "object") return { ok: false };
  if (fwTruthyId(body.event_id)) return { ok: true, reason: "event_id" };
  const docId = body.document_id ?? body.doc_id ?? body.documentId;
  if (fwTruthyId(docId)) return { ok: true, reason: "document_id" };
  if (hasMatchedPortalRecord(body)) return { ok: true, reason: "matched_record" };
  if (hasExplicitDurableSuccess(body)) return { ok: true, reason: "explicit_success" };
  return { ok: false };
}

function inferMatchedSummary(body) {
  if (!body || typeof body !== "object") return false;
  if (hasMatchedPortalRecord(body)) return true;
  if (body.matched === true) return true;
  return false;
}

function extractFwUploadDecisionFields(body, httpStatus, httpOk) {
  const b = body && typeof body === "object" ? body : {};
  const hasSuccess = Object.prototype.hasOwnProperty.call(b, "success");
  return {
    http_status: httpStatus,
    http_ok: httpOk,
    success: hasSuccess ? b.success : undefined,
    event_id: b.event_id,
    document_id: b.document_id ?? b.doc_id ?? b.documentId,
    matched: inferMatchedSummary(b),
    unmatched:
      b.unmatched === true ||
      String(b.match_status || b.document_match_status || "")
        .toLowerCase()
        .includes("unmatch"),
    rejected:
      b.rejected === true ||
      String(b.status || "")
        .toLowerCase()
        .includes("reject"),
    message: b.message || b.error || "",
    match_status: b.match_status || b.document_match_status,
  };
}

/**
 * Portal truth for moving a file into FileWisely Processed — not HTTP 200 or non-empty JSON alone.
 * @returns {{ classification: string, processedConfirmed: boolean, processedReason: string|null, notConfirmedReason: string|null, fields: object }}
 */
function classifyFilewiselyIngestOutcome(body, httpStatus, httpOk) {
  const fields = extractFwUploadDecisionFields(body, httpStatus, httpOk);
  if (!httpOk) {
    return {
      classification: "failed",
      processedConfirmed: false,
      processedReason: null,
      notConfirmedReason: `http_${httpStatus}`,
      fields,
    };
  }
  const b = body && typeof body === "object" ? body : {};
  if (
    b.rejected === true ||
    String(b.status || "").toLowerCase() === "rejected" ||
    (b.accepted === false && b.success === false)
  ) {
    return {
      classification: "rejected",
      processedConfirmed: false,
      processedReason: null,
      notConfirmedReason: "rejected_by_backend",
      fields,
    };
  }
  const conf = getPortalProcessedConfirmation(b);
  if (conf.ok) {
    return {
      classification: "confirmed_processed",
      processedConfirmed: true,
      processedReason: conf.reason,
      notConfirmedReason: null,
      fields,
    };
  }
  if (b.unmatched === true) {
    return {
      classification: "uploaded_but_unmatched",
      processedConfirmed: false,
      processedReason: null,
      notConfirmedReason: "unmatched_flag",
      fields,
    };
  }
  const ms = String(b.match_status || b.document_match_status || "").toLowerCase();
  if (ms === "unmatched" || ms === "no_match" || ms === "none") {
    return {
      classification: "uploaded_but_unmatched",
      processedConfirmed: false,
      processedReason: null,
      notConfirmedReason: "match_status_unmatched",
      fields,
    };
  }
  return {
    classification: "uploaded_but_unconfirmed",
    processedConfirmed: false,
    processedReason: null,
    notConfirmedReason: "no_portal_confirmation_signals",
    fields,
  };
}

function safeJsonStringify(x) {
  try {
    return JSON.stringify(x);
  } catch (_) {
    return String(x);
  }
}

function buildFwNonConfirmedSidecar(fwDecision, rawResponse, extra = null) {
  const o = {
    outcome: fwDecision.classification,
    not_confirmed_reason: fwDecision.notConfirmedReason,
    processed_confirmed: fwDecision.processedConfirmed,
    processed_reason: fwDecision.processedReason,
    decision_fields: fwDecision.fields,
    response_excerpt: truncateFwPipelineDetail(safeJsonStringify(rawResponse)),
  };
  if (extra && typeof extra === "object") Object.assign(o, extra);
  return JSON.stringify(o);
}

function logFwUploadDecisionFields(path, fields) {
  const succ =
    fields.success === undefined ? "n/a" : String(fields.success);
  const msg = truncateFwPipelineDetail(String(fields.message || ""), 400);
  console.info(
    `[UCE] FW_UPLOAD_DECISION_FIELDS path=${path} http_status=${fields.http_status} success=${succ} event_id=${fields.event_id ?? "n/a"} document_id=${fields.document_id ?? "n/a"} matched=${fields.matched} unmatched=${fields.unmatched} rejected=${fields.rejected} message=${msg}`
  );
}

async function uploadCapture(
  capturePayload,
  contextSnapshot,
  selectedCaptureMode,
  captureSource = "unknown",
  fwPipelinePath = null
) {
  const sourceApp =
    contextSnapshot?.source_app || capturePayload?.source_app || "unknown";
  const windowTitle =
    contextSnapshot?.window_title || capturePayload?.window_title || "unknown";
  const matchedRule =
    contextSnapshot?.matched_rule || capturePayload?.matched_rule || "none";
  const bucket = contextSnapshot?.bucket || "unknown";
  const actionAllowed = contextSnapshot?.action_allowed === true;
  const preferredCaptureMode =
    contextSnapshot?.preferred_capture_mode || "screenshot";
  const workflowKind = contextSnapshot?.workflow_kind || "unknown";
  const finalSelectedMode = selectedCaptureMode || "screenshot";
  const isPdf = finalSelectedMode === "pdf";
  const docSubtype = resolveDocumentSubtype(matchedRule, windowTitle);
  const documentType = resolveDocumentType(matchedRule, isPdf, windowTitle);
  const capturedAtIso = capturePayload?.captured_at_unix_ms
    ? new Date(capturePayload.captured_at_unix_ms).toISOString()
    : new Date().toISOString();

  const pathForPayload =
    (fwPipelinePath && String(fwPipelinePath)) ||
    capturePayload?.file_path ||
    "";

  if (fwPipelinePath) {
    await logPipeline("MATCHING_STARTED", fwPipelinePath, {
      matched_rule: matchedRule,
      workflow_kind: workflowKind,
      bucket,
      window_title: windowTitle,
      file_path: pathForPayload,
      capture_source: captureSource,
    });
  }

  const bid = getBusinessId();
  if (BACKEND_UPLOAD_URL && !bid) {
    lastUploadDebug = {
      at: new Date().toISOString(),
      endpoint: BACKEND_UPLOAD_URL,
      payload: null,
      response: {
        success: false,
        status: "missing_tenant",
        message:
          "Set business_id: uce-tenant.json in app data, or VITE_UCE_BUSINESS_ID at build time.",
      },
    };
    throw new Error(
      "Filewisely: no business_id for this install — configure tenant for this shop."
    );
  }

  const payload = {
    business_id: bid,
    image_base64: isPdf ? "" : capturePayload?.image_base64 || "",
    file_base64: isPdf ? capturePayload?.image_base64 || "" : "",
    source_app: sourceApp,
    window_title: windowTitle,
    matched_rule: matchedRule,
    workflow_kind: workflowKind,
    bucket,
    action_allowed: actionAllowed,
    preferred_capture_mode: preferredCaptureMode,
    selected_capture_mode: finalSelectedMode,
    document_type: documentType,
    captured_at: capturedAtIso,
    device_id: getDeviceId(),
    file_path: pathForPayload,
    context: {
      source_app: sourceApp,
      window_title: windowTitle,
    },
    classification: {
      matched_rule: matchedRule,
      workflow_kind: workflowKind,
      bucket,
      action_allowed: actionAllowed,
      preferred_capture_mode: preferredCaptureMode,
      selected_capture_mode: finalSelectedMode,
    },
    event_meta: {
      captured_at: capturedAtIso,
      device_id: getDeviceId(),
      file_path: pathForPayload,
      document_type: documentType,
      file_encoding: isPdf ? "base64_pdf" : "base64_image",
      workflow_kind: workflowKind,
    },
    metadata: {
      changed: contextSnapshot?.changed ?? false,
      in_cooldown: contextSnapshot?.in_cooldown ?? false,
      capture_message: capturePayload?.message || "",
      app_version: "uce-universal-capture-engine-v1",
      capture_source: captureSource,
      desktop_document_subtype: docSubtype || "unknown",
    },
  };

  if (!BACKEND_UPLOAD_URL) {
    if (fwPipelinePath) {
      await logPipeline("UPLOAD_REQUEST_SKIPPED", fwPipelinePath, {
        reason: "no_backend_url",
      });
    }
    lastUploadDebug = {
      at: new Date().toISOString(),
      endpoint: null,
      payload,
      response: { success: true, status: "skipped", message: "No endpoint configured" },
    };
    lastSuccessfulUploadAt = Date.now();
    return { ok: true, skipped: true, message: "Capture saved locally." };
  }

  const controller = new AbortController();
  const timeoutId = setTimeout(() => controller.abort(), UPLOAD_TIMEOUT_MS);

  let response;
  try {
    if (fwPipelinePath) {
      await logPipeline("UPLOAD_REQUEST_STARTED", fwPipelinePath, {
        endpoint: BACKEND_UPLOAD_URL,
      });
    }
    response = await fetch(BACKEND_UPLOAD_URL, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${SUPABASE_ANON_KEY}`,
        apikey: SUPABASE_ANON_KEY,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(payload),
      signal: controller.signal,
    });
  } catch (error) {
    if (fwPipelinePath) {
      await logPipeline("UPLOAD_REQUEST_FINISHED", fwPipelinePath, {
        http_status: null,
        error: true,
        name: error?.name,
        message: error?.message || String(error),
      });
    }
    let endpointHost = "unknown-endpoint";
    try {
      endpointHost = new URL(BACKEND_UPLOAD_URL).host;
    } catch (_) {
      // Ignore URL parse errors for fallback messaging.
    }
    if (error?.name === "AbortError") {
      throw new Error("Upload timed out after 10s");
    }
    throw new Error(
      `Upload request failed (${endpointHost}). Check endpoint path/deployment and CORS.`
    );
  } finally {
    clearTimeout(timeoutId);
  }

  let responseBody = {};
  try {
    responseBody = await response.json();
  } catch (_) {
    // Intentionally ignore non-JSON response payloads.
  }

  if (fwPipelinePath) {
    await logPipeline("UPLOAD_REQUEST_FINISHED", fwPipelinePath, {
      http_status: response.status,
      ok: response.ok,
    });
    let bodyStr = "";
    try {
      bodyStr = JSON.stringify(responseBody);
    } catch (_) {
      bodyStr = String(responseBody);
    }
    await logPipeline("UPLOAD_RESPONSE_BODY", fwPipelinePath, {
      body: truncateFwPipelineDetail(bodyStr),
    });
    await logPipeline("MATCHING_RESULT", fwPipelinePath, {
      http_ok: response.ok,
      status: responseBody?.status,
      event_id: responseBody?.event_id,
      message: responseBody?.message,
      rejected: responseBody?.rejected,
      unmatched: responseBody?.unmatched,
    });
  }

  lastUploadDebug = {
    at: new Date().toISOString(),
    endpoint: BACKEND_UPLOAD_URL,
    payload,
    http_status: response.status,
    response: responseBody,
  };

  if (!response.ok) {
    const errorMessage =
      responseBody?.message || `Upload failed (${response.status})`;
    throw new Error(errorMessage);
  }

  lastSuccessfulUploadAt = Date.now();
  return responseBody;
}

async function runTrainFromUi() {
  if (busy) return;
  const wf = getTrainWorkflowTarget();
  uceTrainBtn.disabled = true;
  try {
    const result = await invoke("train_workflow_context", { workflow: wf });
    showToast(
      result || `Context trained as ${workflowTrainLabel(wf)}`,
      "success"
    );
    await refreshContextState();
  } catch (error) {
    const message =
      typeof error === "string"
        ? error
        : error?.message || "Training failed";
    showToast(message, "error");
  } finally {
    uceTrainBtn.disabled = false;
  }
}

async function runForgetTrainingFromUi() {
  if (busy) return;
  uceTrainBtn.disabled = true;
  try {
    const result = await invoke("forget_ccc_training_for_current_context");
    showToast(result || "Training updated", "success");
    await refreshContextState();
  } catch (error) {
    const message =
      typeof error === "string"
        ? error
        : error?.message || "Could not remove training";
    showToast(message, "error");
  } finally {
    uceTrainBtn.disabled = false;
  }
}

async function runExcludeCurrentFromUi() {
  if (busy) return;
  uceTrainBtn.disabled = true;
  try {
    const result = await invoke("exclude_ccc_context_for_current_window");
    showToast(result || "Context excluded", "success");
    await refreshContextState();
  } catch (error) {
    const message =
      typeof error === "string"
        ? error
        : error?.message || "Exclude failed";
    showToast(message, "error");
  } finally {
    uceTrainBtn.disabled = false;
  }
}

async function runClearExcludeFromUi() {
  if (busy) return;
  uceTrainBtn.disabled = true;
  try {
    const result = await invoke("clear_ccc_excludes_for_current_window");
    showToast(result || "Exclude cleared", "success");
    await refreshContextState();
  } catch (error) {
    const message =
      typeof error === "string"
        ? error
        : error?.message || "Could not clear exclude";
    showToast(message, "error");
  } finally {
    uceTrainBtn.disabled = false;
  }
}

async function handleCapture() {
  if (busy) return;
  busy = true;
  uceBtn.classList.add("busy");
  uceTrainBtn.disabled = true;
  try {
    let contextSnapshot = lastPolledContext || {};
    if (!contextSnapshot || Object.keys(contextSnapshot).length === 0) {
      try {
        contextSnapshot = await invoke("get_last_observed_context");
      } catch (_) {
        contextSnapshot = {};
      }
    }
    triggerFlash();
    const capture = await invoke("capture_screen");
    const selectedMode =
      preferredCaptureModeFromUceDetection(getLastUceDetectedContext()) ||
      resolvePreferredMode(contextSnapshot);
    const uploadResult = await uploadCapture(
      capture,
      contextSnapshot,
      selectedMode,
      "button_screenshot"
    );
    const status = uploadResult?.status ? ` (${uploadResult.status})` : "";
    const eventId = uploadResult?.event_id ? ` #${uploadResult.event_id}` : "";
    const message = uploadResult?.message || "Capture uploaded";
    if (!uploadResult?.skipped) {
      emitUceContextCapturedIfRelevant({
        source: "button_capture",
        fileHint: null,
      });
    }
    showToast(`${message}${status}${eventId}`, "success");
  } catch (error) {
    console.error("Capture flow error:", error);
    const errorText =
      typeof error === "string"
        ? error
        : error?.message || error?.toString?.() || "Capture failed";
    showToast(errorText, "error");
  } finally {
    busy = false;
    uceBtn.classList.remove("busy");
    uceTrainBtn.disabled = false;
  }
}

async function getUploadContextSnapshot() {
  if (lastPolledContext) return lastPolledContext;
  try {
    return await invoke("get_last_observed_context");
  } catch (_) {
    return {};
  }
}

/**
 * CCC often prints several PDFs in a burst; the watcher emits once per file. Without debouncing,
 * the first `checkAutoPdfUpload` sets `autoPdfBusy` and the rest return immediately — files are missed.
 * Wait INCOMING_BATCH_DEBOUNCE_MS after the last signal, then run one upload pass over the full list.
 */
function scheduleIncomingAutoPdfUpload() {
  if (incomingAutoPdfDebounceTimer) clearTimeout(incomingAutoPdfDebounceTimer);
  incomingAutoPdfDebounceTimer = setTimeout(() => {
    incomingAutoPdfDebounceTimer = null;
    void checkAutoPdfUpload("incoming-debounced");
  }, INCOMING_BATCH_DEBOUNCE_MS);
}

async function checkAutoPdfUpload(source = "poll") {
  if (autoPdfBusy) {
    if (source === "incoming-debounced" || source === "incoming-queued") {
      autoPdfIncomingPending = true;
      console.info(
        "[UCE] Auto PDF: busy — queued follow-up run (incoming burst while uploading)"
      );
    }
    return;
  }
  const ctxSnap = await getUploadContextSnapshot();
  const fromIncomingFileEvent =
    source === "incoming-debounced" || source === "incoming-queued";

  const list = await invoke("list_pdf_metas_since", {
    sinceUnixMs: autoPdfSinceUnixMs,
  });
  const listArr = Array.isArray(list) ? list : [];
  /** Every `fw_*.pdf` in FileWisely Incoming (no `since` filter) — Word→PDF can have mtime < app start. */
  let mergedList = listArr;
  try {
    const fwOnly = await invoke("list_fw_pdf_metas_in_filewisely_incoming");
    if (Array.isArray(fwOnly) && fwOnly.length > 0) {
      const byKey = new Map();
      for (const m of listArr) {
        if (m?.file_path) byKey.set(fwKeyPath(m.file_path), m);
      }
      for (const m of fwOnly) {
        if (!m?.file_path || !isFwPdfPath(m.file_path)) continue;
        const k = fwKeyPath(m.file_path);
        if (!byKey.has(k)) byKey.set(k, m);
      }
      mergedList = Array.from(byKey.values());
      mergedList.sort((a, b) => {
        const ta = Number(a?.modified_unix_ms) || 0;
        const tb = Number(b?.modified_unix_ms) || 0;
        if (ta !== tb) return ta - tb;
        return String(a?.file_path || "").localeCompare(String(b?.file_path || ""));
      });
    }
  } catch (e) {
    console.warn("[UCE] merge fw Incoming list failed:", e);
    mergedList = listArr;
  }

  const fwBypass =
    mergedList.length > 0 &&
    mergedList.some((m) => m?.file_path && isFwPdfPath(m.file_path));
  if (
    !fromIncomingFileEvent &&
    !fwBypass &&
    resolvePreferredMode(ctxSnap || {}) !== "pdf"
  ) {
    if (String(source).startsWith("incoming")) {
      console.info(
        "[UCE] Auto PDF skipped: not PDF workflow context (source=" + source + ")"
      );
    }
    return;
  }

  if (mergedList.length === 0) {
    if (fromIncomingFileEvent) {
      console.info(
        `[UCE] Auto PDF list empty (source=${source}) since=${autoPdfSinceUnixMs}`
      );
    }
    return;
  }

  autoPdfBusy = true;
  try {
    const pending = mergedList.filter((meta) => {
      if (!meta?.file_path) return false;
      if (isFwPdfPath(meta.file_path)) {
        const st = getFwState(meta.file_path);
        st.discoveredOnDisk = true;
        if (st.uploadFinished) return false;
      }
      const fp = `${meta.file_path}|${meta.modified_unix_ms}`;
      if (uploadedPdfFingerprints.has(fp)) {
        if (isFwPdfPath(meta.file_path)) {
          console.info(`[UCE] trace js_dedupe_skipped path=${meta.file_path}`);
        }
        return false;
      }
      return true;
    });
    if (pending.length === 0) {
      console.info(
        `[UCE] Auto PDF: all ${mergedList.length} file(s) already in fingerprint set`
      );
      return;
    }

    console.info(
      `[UCE] Auto PDF batch (source=${source}) list=${mergedList.length} pending=${pending.length}`
    );

    const sentNames = [];
    const errors = [];
    /** Cap per tick so a huge Incoming folder cannot block the UI for minutes. */
    const MAX_AUTO_PDF_PER_RUN = 40;
    const batch = pending.slice(0, MAX_AUTO_PDF_PER_RUN);
    const batchPaths = batch.map((b) => b.file_path).filter(Boolean);
    console.info(
      `[UCE] Upload batch count=${batch.length} files=[${batchPaths.join(", ")}]`
    );
    let lastUploadResult = null;
    /** Dedupe within batch by full path + mtime (not basename-only). */
    const seenFpThisBatch = new Set();

    for (const newest of batch) {
      const key = `${newest.file_path}|${newest.modified_unix_ms}`;
      const fp = key;
      if (uploadedPdfFingerprints.has(fp)) continue;
      if (seenFpThisBatch.has(fp)) {
        if (isFwPdfPath(newest.file_path)) {
          console.info(
            `[UCE] trace js_dedupe_skipped path=${newest.file_path}`
          );
        }
        continue;
      }
      seenFpThisBatch.add(fp);

      if (isFwPdfPath(newest.file_path)) {
        console.info(
          `[UCE] trace js_queued_for_upload path=${newest.file_path}`
        );
        const fwSrc = inferFwUploadSource(newest.file_path, source);
        try {
          const r = await queueFwUpload(newest.file_path, newest, fwSrc);
          lastUploadResult = r?.uploadResult ?? lastUploadResult;
          lastAutoPdfKey = key;
          if (r?.pdfName && r?.uploadResult && !r.uploadResult?.skipped) {
            sentNames.push(r.pdfName);
            if (source === "backup") {
              logEvent("upload_retry", r.pdfName);
            }
          }
        } catch (oneErr) {
          const pdfName =
            newest.file_path.split(/[\\/]/).pop() || "file.pdf";
          errors.push({ name: pdfName, err: oneErr });
          console.error(`[UCE] Upload failed: ${pdfName}`, oneErr);
        }
        continue;
      }

      try {
        try {
          await invoke("uce_log_pdf_lifecycle", {
            phase: "upload_started",
            path: newest.file_path,
          });
        } catch (_) {
          /* optional command */
        }
        const capture = await invoke("read_pdf_file", { path: newest.file_path });
        const contextSnapshot = await getUploadContextSnapshot();
        const uploadResult = await uploadCapture(
          capture,
          contextSnapshot,
          "pdf",
          "auto_pdf_folder"
        );
        try {
          await invoke("uce_log_pdf_lifecycle", {
            phase: "upload_finished",
            path: newest.file_path,
            success: !uploadResult?.skipped,
          });
        } catch (_) {
          /* optional command */
        }
        lastUploadResult = uploadResult;
        lastAutoPdfKey = key;
        lastAutoPdfUploadAt = Date.now();
        if (!uploadResult?.skipped) {
          uploadedPdfFingerprints.add(fp);
          emitUceContextCapturedIfRelevant({
            source: "auto_pdf_folder",
            fileHint:
              newest.file_path.split(/[\\/]/).pop() || "recent.pdf",
          });
        }
        const pdfName = newest.file_path.split(/[\\/]/).pop() || "recent.pdf";
        if (uploadResult?.skipped) {
          console.info(`[UCE] Upload skipped (no endpoint / local): ${pdfName}`);
        } else {
          console.info(`[UCE] Upload success: ${pdfName}`);
        }
        if (source === "backup") {
          logEvent("upload_retry", pdfName);
        }
        sentNames.push(pdfName);
      } catch (oneErr) {
        try {
          await invoke("uce_log_pdf_lifecycle", {
            phase: "upload_finished",
            path: newest.file_path,
            success: false,
          });
        } catch (_) {
          /* optional command */
        }
        const pdfName = newest.file_path.split(/[\\/]/).pop() || "file.pdf";
        errors.push({ name: pdfName, err: oneErr });
        console.error(`[UCE] Upload failed: ${pdfName}`, oneErr);
      }
    }

    if (sentNames.length > 0) {
      if (sentNames.length === 1) {
        const st = lastUploadResult?.status ? ` (${lastUploadResult.status})` : "";
        const ev = lastUploadResult?.event_id
          ? ` #${lastUploadResult.event_id}`
          : "";
        showToast(`Uploaded: ${sentNames[0]}${st}${ev}`, "success");
      } else {
        const preview = sentNames.slice(0, 4).join(", ");
        const more =
          sentNames.length > 4 ? ` (+${sentNames.length - 4} more)` : "";
        showToast(
          `Uploaded ${sentNames.length} files: ${preview}${more}`,
          "success",
          5500
        );
      }
    }
    if (errors.length > 0) {
      const first = errors[0];
      const errorText =
        typeof first.err === "string"
          ? first.err
          : first.err?.message ||
            first.err?.toString?.() ||
            "Upload failed";
      showToast(`${first.name}: ${errorText}`, "error");
    }
  } catch (error) {
    console.error("Auto PDF upload error:", error);
    const errorText =
      typeof error === "string"
        ? error
        : error?.message || error?.toString?.() || "Upload failed";
    showToast(errorText, "error");
  } finally {
    autoPdfBusy = false;
    void updateUceHealthStrip();
    if (autoPdfIncomingPending) {
      autoPdfIncomingPending = false;
      setTimeout(() => void checkAutoPdfUpload("incoming-queued"), 0);
    }
  }
}

function setUiState(nextState) {
  if (currentUiState === nextState) return;
  currentUiState = nextState;
  uceBtn.classList.remove("quiet", "active");
  uceBtn.classList.add(nextState);
}

async function syncWatchPolicyFromRemote() {
  if (!WATCH_POLICY_URL || !getBusinessId()) return;
  try {
    const msg = await invoke("sync_watch_policy_from_remote", {
      url: WATCH_POLICY_URL,
      business_id: getBusinessId(),
      authorization: SUPABASE_ANON_KEY || null,
    });
    console.info("[UCE]", msg);
    await refreshContextState();
  } catch (e) {
    console.error("[UCE] watch policy sync failed:", e);
  }
}

async function refreshContextState() {
  try {
    const ctx = await invoke("get_watch_context");
    lastPolledContext = ctx;
    applyActiveWindowRoMonitor(ctx);
    recordCccSeenFromContext(ctx);
    if (ctx?.matched === true || inferPreferredModeFromContext(ctx) === "pdf") {
      lastKnownContext = ctx;
      lastKnownContextAt = Date.now();
    }
    const isActive = ctx?.action_allowed === true;
    setUiState(isActive ? "active" : "quiet");
    const now = Date.now();
    const signals = getUceRecognitionSignals();
    const rawDetected = detectUceContext(ctx, signals);
    const step = stepUceOperatorState(rawDetected, now, signals);
    const reasonPayload = buildUceReasonPayload({
      detected: step.detected,
      opportunity: step.opportunity,
      rawDetected: step.rawDetected,
      contextEnteredAt: step.contextEnteredAt,
      signals,
      now,
      windowTitle: ctx?.window_title,
    });
    lastUceDecisionReasons = reasonPayload.decisionReasons || [];
    const resolvedMode =
      preferredCaptureModeFromUceDetection(step.detected) ||
      inferPreferredModeFromContext(ctx);
    applyUceFloatingButtonChrome(uceBtn, step.detected, {
      pulseOnce: pulseCaptureCueOnce,
      opportunity: step.opportunity,
      resolvedCaptureMode: resolvedMode,
      modeBadgeEl: uceCaptureModeBadge,
      now,
      reason: reasonPayload,
    });
    maybeEmitUceContextForTheo(
      step.detected,
      getMonitoredCurrentRo(),
      ctx?.window_title,
      reasonPayload
    );
    void refreshRoStatusAfterContext();
    void updateUceHealthStrip();
  } catch (error) {
    console.error("Context refresh error:", error);
    setUiState("quiet");
    clearRoLevelClasses();
    lastUceDecisionReasons = [];
    stepUceOperatorState(null, Date.now(), {});
    maybeEmitUceContextForTheo(null, null, null, null);
    applyUceFloatingButtonChrome(uceBtn, null, {
      pulseOnce: pulseCaptureCueOnce,
      opportunity: { type: "idle", context: "unknown" },
      modeBadgeEl: uceCaptureModeBadge,
      now: Date.now(),
    });
  }
}

function formatUploadDebugBlock() {
  if (!lastUploadDebug) {
    return "Last upload:\n(none yet — capture/upload something first)";
  }
  const status = lastUploadDebug?.response?.status || "unknown";
  const eventId = lastUploadDebug?.response?.event_id
    ? `#${lastUploadDebug.response.event_id}`
    : "no-event";
  const message =
    lastUploadDebug?.response?.message || "No backend message";
  const sentPath =
    lastUploadDebug?.payload?.file_path ||
    lastUploadDebug?.payload?.event_meta?.file_path ||
    "none";
  const sentFile = sentPath.split(/[\\/]/).pop() || sentPath;
  return `Last upload:\n${status} ${eventId}\n${message}\nfile: ${sentFile}`;
}

/** Plain right-click: context + last upload debug + optional RO status panel (see UCE_AWARENESS_LAYER_SPEC). */
async function showFullRightClickDebugToast() {
  let ctx = lastPolledContext;
  if (!ctx) {
    try {
      ctx = await invoke("get_last_observed_context");
    } catch (error) {
      console.error("Debug fallback read error:", error);
    }
  }

  const tid = getBusinessId();
  const tenantLine = tid
    ? `Tenant: ${tid.length > 24 ? `${tid.slice(0, 10)}…${tid.slice(-8)}` : tid}`
    : "Tenant: (not set — configure uce-tenant.json or VITE_UCE_BUSINESS_ID)";

  const lines = [tenantLine, "---"];
  if (ctx) {
    const app = ctx?.source_app || "unknown";
    const rule = ctx?.matched_rule || "none";
    const wf = ctx?.workflow_kind || "?";
    const cooldown = ctx?.in_cooldown ? "cooldown" : "ready";
    const title = (ctx?.window_title || "").trim() || "no-title";
    lines.push(
      `Context: ${app} | ${rule} | wf:${wf} | ${cooldown}`,
      title,
      "---",
      formatUploadDebugBlock()
    );
    const det = getLastUceDetectedContext();
    const opp = getLastUceOpportunity();
    const opDbg = getUceOperatorDebugSnapshot(Date.now());
    if (det) {
      const rawS = opDbg.rawDetectedContext
        ? `${opDbg.rawDetectedContext.type}/${opDbg.rawDetectedContext.bucket}@${opDbg.rawDetectedContext.confidence.toFixed(2)}`
        : "(none)";
      const propS = opDbg.proposedContext
        ? `${opDbg.proposedContext.type}/${opDbg.proposedContext.bucket}@${opDbg.proposedContext.confidence.toFixed(2)}`
        : "(none)";
      const stabS = opDbg.lastStableContext
        ? `${opDbg.lastStableContext.type}/${opDbg.lastStableContext.bucket}@${opDbg.lastStableContext.confidence.toFixed(2)}`
        : "(none)";
      lines.push(
        "---",
        `UCE context v1: ${det.type} | ${det.bucket} | conf ${det.confidence.toFixed(2)} | mode ${det.preferredCaptureMode}`,
        `Opportunity: ${opp.type} (${opp.context}) | esc +${opDbg.escalationTicks} ticks / ${opDbg.escalationHeldMs}ms (each ${opDbg.escalationIntervalMs}ms) | pause≥${opDbg.userPauseMs}ms`,
        `Active hold window: ${opDbg.lastCommittedHoldMs ?? "n/a"}ms | known floor ${opDbg.knownConfidenceFloor}`,
        `Stable: ${stabS} | raw: ${rawS} | proposed: ${propS}`,
        `Hold remaining: ${opDbg.holdRemainingMs}ms | downgradeGrace: ${opDbg.downgradeGraceActive ? `yes (${opDbg.downgradeGraceRemainingMs}ms)` : "no"}`,
        `decisionReasons: ${(lastUceDecisionReasons || []).join(", ") || "(none)"}`,
        `pendingMissingSoft: ${isPendingMissingSuppressActive() ? `yes (until ${new Date(getSuppressAggressiveMissingUntilMs()).toLocaleTimeString()})` : "no"}`
      );
    }
  } else {
    lines.push("Context: (none yet)", "---", formatUploadDebugBlock());
  }

  const roResolved = resolveRoForAwareness();
  const roUrl = getRoStatusUrl();
  const roNum = roResolved.ro;
  let roPayload = null;
  lines.push("---", `current_ro: ${roNum || "(none)"}`);

  if (roUrl && tid && roNum) {
    try {
      roPayload = await fetchRoStatus(roNum, true, roResolved.windowTitleForApi);
    } catch (e) {
      roPayload = {
        _error:
          typeof e === "string"
            ? e
            : e?.message || e?.toString?.() || "RO status request failed",
      };
    }
  }

  debugSheetEl.classList.remove("uce-debug-sheet--prod");
  debugSheetEl.replaceChildren();
  const pre = document.createElement("pre");
  pre.className = "uce-debug-pre";
  pre.textContent = lines.filter(Boolean).join("\n");
  debugSheetEl.appendChild(pre);
  if (roUrl && tid && roNum) {
    debugSheetEl.appendChild(buildRoStatusPanel(roNum, roPayload));
  } else if (tid) {
    debugSheetEl.appendChild(buildRoStatusUnavailablePanel(roUrl, roNum));
  }

  await pickRightPeekSide();
  appEl.classList.add("uce-debug-open");
  debugSheetEl.hidden = false;
  debugSheetEl.setAttribute("aria-hidden", "false");
  await fitRightPeekLayout();
  peekSidePanelMode = "debug";
}

uceBtn.addEventListener("pointerdown", (event) => {
  if (event.button === 2) {
    if (event.ctrlKey) {
      rightPeekActive = true;
      void (async () => {
        await showFullRightClickDebugToast();
        if (!rightPeekActive) void dismissRightPeek();
      })();
      return;
    }
    rightPeekActive = true;
    void (async () => {
      await showProductionRoPanel();
      if (!rightPeekActive) void dismissRightPeek();
    })();
    return;
  }
  if (event.button !== 0 || busy) return;
  pointerShiftPressed = event.shiftKey === true;
  pointerIsDown = true;
  dockNativeDragStarted = false;
  updateDockHoverOpacity(event.clientX, event.clientY);
  if (dockDragTimer) clearTimeout(dockDragTimer);
  dockDragTimer = setTimeout(() => {
    if (pointerIsDown && !dockNativeDragStarted) {
      dockNativeDragStarted = true;
      updateDockHoverOpacity(lastPointerClientX, lastPointerClientY);
      invoke("start_window_drag").catch((err) => {
        console.error("start_window_drag:", err);
      });
    }
  }, 130);
});

uceBtn.addEventListener("pointerup", async (event) => {
  if (event.button !== 0) return;
  if (dockDragTimer) {
    clearTimeout(dockDragTimer);
    dockDragTimer = null;
  }
  const wasNativeDrag = dockNativeDragStarted;
  const shouldCapture = pointerIsDown && !dockNativeDragStarted;
  pointerIsDown = false;
  dockNativeDragStarted = false;
  pointerShiftPressed = false;

  if (wasNativeDrag) {
    await snapWindowToNearestCornerZone();
    await saveCurrentPosition();
    updateDockHoverOpacity(lastPointerClientX, lastPointerClientY);
    return;
  }

  if (shouldCapture) {
    await handleCapture();
  }
  updateDockHoverOpacity(lastPointerClientX, lastPointerClientY);
});

uceBtn.addEventListener("pointercancel", () => {
  if (dockDragTimer) {
    clearTimeout(dockDragTimer);
    dockDragTimer = null;
  }
  pointerIsDown = false;
  dockNativeDragStarted = false;
  pointerShiftPressed = false;
  updateDockHoverOpacity(lastPointerClientX, lastPointerClientY);
});

uceBtn.addEventListener("contextmenu", (event) => {
  event.preventDefault();
});

uceTrainBtn.addEventListener("click", (e) => {
  e.stopPropagation();
  if (e.altKey) {
    const next = cycleTrainWorkflowTarget();
    showToast(`Training target: ${workflowTrainLabel(next)}`, "success", 3500);
    return;
  }
  if (e.shiftKey && e.ctrlKey) {
    void runExcludeCurrentFromUi();
  } else if (e.ctrlKey && !e.shiftKey) {
    void runClearExcludeFromUi();
  } else if (e.shiftKey) {
    void runForgetTrainingFromUi();
  } else {
    void runTrainFromUi();
  }
});

uceTrainBtn.addEventListener("contextmenu", (e) => {
  e.preventDefault();
});

uceSettingsBtn.addEventListener("click", (e) => {
  e.stopPropagation();
  const next = !getTrainButtonVisible();
  setTrainButtonVisible(next);
  showToast(
    next
      ? "Training (T) shown — use on each app or site you need, then hide when stable."
      : "Training (T) hidden — open again anytime (e.g. new DMS or browser workflow).",
    "success",
    4000
  );
});

uceSettingsBtn.addEventListener("contextmenu", (e) => {
  e.preventDefault();
});

applyTrainButtonVisibility();
initUceQaAutomation();

uceRoPanelBtn.addEventListener("click", (e) => {
  e.stopPropagation();
  if (appEl.classList.contains("uce-debug-open") && peekSidePanelMode === "production") {
    void dismissRightPeek();
    return;
  }
  void showProductionRoPanel();
});

function endRightPeekIfActive() {
  if (!rightPeekActive) return;
  void dismissRightPeek();
}

document.addEventListener(
  "pointerup",
  (e) => {
    if (e.button !== 2) return;
    endRightPeekIfActive();
  },
  { capture: true }
);
document.addEventListener(
  "pointercancel",
  () => {
    endRightPeekIfActive();
  },
  { capture: true }
);

document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if (appEl.classList.contains("uce-printer-severe-open")) {
    e.preventDefault();
    void hidePrinterSevereModal(false);
    return;
  }
  if (appEl.classList.contains("uce-tenant-setup-open")) {
    e.preventDefault();
    return;
  }
  if (appEl.classList.contains("uce-debug-open")) {
    e.preventDefault();
    void dismissRightPeek();
    return;
  }
  if (!toastEl.className.includes("show")) return;
  e.preventDefault();
  rightPeekActive = false;
  dismissToast();
});

new ResizeObserver(() => {
  if (toastEl.className.includes("show")) {
    scheduleFitWindowToToast();
  }
}).observe(toastEl);

if (uceDockEl) {
  new ResizeObserver(() => {
    scheduleCompactResizeFromDockMeasure();
  }).observe(uceDockEl);
}
if (uceBlockingBanner) {
  new ResizeObserver(() => {
    scheduleCompactResizeFromDockMeasure();
  }).observe(uceBlockingBanner);
}

requestAnimationFrame(() => {
  requestAnimationFrame(() => {
    if (shouldMeasureDomForCompactWindow()) void setCompactWindowSize();
  });
});

/** GitHub Releases + Tauri updater. (Was 6h — restarts within the window never checked.) */
const UCE_UPDATE_CHECK_INTERVAL_MS = 15 * 60 * 1000;

/**
 * @param {{ force?: boolean }} [options] — `force: true` skips throttle (for `window.__uceCheckUpdate()`).
 * @returns {Promise<Record<string, unknown>>}
 */
async function runUceAppUpdateCheck(options = {}) {
  const force = options.force === true;
  if (import.meta.env.DEV) {
    return { ok: false, reason: "dev_build" };
  }
  if (!force) {
    const last = Number(localStorage.getItem("uce_last_update_check_ms") || "0");
    const elapsed = Date.now() - last;
    if (last > 0 && elapsed < UCE_UPDATE_CHECK_INTERVAL_MS) {
      return {
        ok: false,
        reason: "throttled",
        msUntilNextCheck: UCE_UPDATE_CHECK_INTERVAL_MS - elapsed,
      };
    }
  }
  let update;
  try {
    const { check } = await import("@tauri-apps/plugin-updater");
    const { relaunch } = await import("@tauri-apps/plugin-process");
    update = await check();
    localStorage.setItem("uce_last_update_check_ms", String(Date.now()));
    if (!update) {
      return { ok: true, reason: "already_latest" };
    }
    await update.downloadAndInstall();
    await relaunch();
    return { ok: true, reason: "relaunching" };
  } catch (e) {
    if (update === undefined) {
      console.warn("[UCE] auto-update check failed:", e);
      return { ok: false, reason: "check_failed", error: String(e) };
    }
    console.warn("[UCE] auto-update install failed:", e);
    return { ok: false, reason: "install_failed", error: String(e) };
  }
}

setTimeout(() => {
  void runUceAppUpdateCheck();
}, 8000);

function hideBlockingBanner() {
  if (!uceBlockingBanner) return;
  if (uceBlockingBanner.hidden) return;
  uceBlockingBanner.hidden = true;
  healthBannerVisible = false;
  appEl.classList.remove("uce-blocking-banner-visible");
  void setCompactWindowSize();
}

function showBlockingBanner() {
  if (!uceBlockingBanner) return;
  if (uceBlockingBanner.hidden) {
    logEvent("attention_banner_shown", "documents may not be captured");
  }
  uceBlockingBanner.hidden = false;
  healthBannerVisible = true;
  appEl.classList.add("uce-blocking-banner-visible");
  void setCompactWindowSize();
}

async function hidePrinterSevereModal(fromHealthyRecovery = false) {
  if (!ucePrinterSevereModal) return;
  ucePrinterSevereModal.hidden = true;
  appEl.classList.remove("uce-printer-severe-open");
  if (fromHealthyRecovery) printerModalDismissedUntilOk = false;
  else printerModalDismissedUntilOk = true;
  try {
    await setCompactWindowSize();
  } catch (e) {
    console.error("hidePrinterSevereModal:", e);
  }
}

async function showPrinterSevereModal() {
  if (!ucePrinterSevereModal || printerModalDismissedUntilOk) return;
  if (appEl.classList.contains("uce-tenant-setup-open")) return;
  const firstOpen = ucePrinterSevereModal.hidden;
  ucePrinterSevereModal.hidden = false;
  appEl.classList.add("uce-printer-severe-open");
  if (firstOpen) {
    logEvent("printer_severe_alert", "Printer missing > 2m — auto-repair");
  }
  try {
    await invoke("uce_set_overlay_logical_size", { width: 420, height: 280 });
  } catch (e) {
    console.error("Printer severe modal resize:", e);
  }
}

function syncHardFailureUi(printerOk, uploadsOk) {
  if (appEl.classList.contains("uce-tenant-setup-open")) return;
  const now = Date.now();
  const coreUnhealthy = !printerOk || !uploadsOk;

  if (!printerOk) {
    if (printerMissingSince === null) printerMissingSince = now;
  } else {
    printerMissingSince = null;
    printerModalDismissedUntilOk = false;
    if (ucePrinterSevereModal && !ucePrinterSevereModal.hidden) {
      void hidePrinterSevereModal(true);
    }
  }

  if (coreUnhealthy) {
    if (coreUnhealthySince === null) coreUnhealthySince = now;
  } else {
    coreUnhealthySince = null;
    hideBlockingBanner();
  }

  if (
    coreUnhealthySince !== null &&
    now - coreUnhealthySince >= HEALTH_HARD_ALERT_MS
  ) {
    showBlockingBanner();
  }

  if (
    printerOk ||
    printerMissingSince === null ||
    now - printerMissingSince < HEALTH_HARD_ALERT_MS
  ) {
    return;
  }
  if (!printerModalDismissedUntilOk) {
    void showPrinterSevereModal();
  }
}

document.getElementById("ucePrinterSevereOk")?.addEventListener("click", () => {
  void hidePrinterSevereModal(false);
});

async function refreshPrinterHealthFromBackend() {
  try {
    const r = await invoke("uce_check_filewisely_printer");
    healthPrinterExact = !!r?.filewisely_exact;
    lastPrinterHealthFetch = Date.now();
    return healthPrinterExact;
  } catch {
    return healthPrinterExact;
  }
}

function uploadsHealthSummary() {
  const d = lastUploadDebug;
  if (!d?.response) return { ok: true, label: "No upload yet" };
  const st = d.response?.status;
  if (st === "skipped") return { ok: true, label: "Backend off (local OK)" };
  if (st === "missing_tenant") return { ok: false, label: "Tenant missing" };
  if (d.response?.success === false) return { ok: false, label: "Last upload failed" };
  const http = d.http_status;
  if (typeof http === "number" && http >= 400) {
    return { ok: false, label: `HTTP ${http}` };
  }
  return { ok: true, label: "OK" };
}

function pdfPipelineStale() {
  const mode = inferPreferredModeFromContext(lastPolledContext || {});
  if (mode !== "pdf") return false;
  if (!lastAutoPdfUploadAt) return false;
  return Date.now() - lastAutoPdfUploadAt > 120_000;
}

async function updateUceHealthStrip() {
  if (!uceHealthStrip) return;
  if (Date.now() - lastPrinterHealthFetch > 15_000) {
    await refreshPrinterHealthFromBackend();
  }
  const up = uploadsHealthSummary();
  const stale = pdfPipelineStale();
  const uploadsOk = up.ok && !stale;
  const printerOk = healthPrinterExact;

  if (!printerOk && !lastEdgePrinterMissing) {
    logEvent("printer_missing", "FileWisely Printer not found");
  }
  lastEdgePrinterMissing = !printerOk;

  const printerLine = printerOk
    ? "Printer: OK (FileWisely Printer)"
    : "Printer: missing — repair runs on cooldown";
  const uploadLine = stale
    ? "Uploads (PDF): delayed — no recent auto-send (check folder / context)"
    : `Uploads: ${up.label}`;
  const uceLine = "UCE: running";
  uceHealthStrip.title = `${printerLine}\n${uceLine}\n${uploadLine}`;
  uceHealthStrip.setAttribute(
    "aria-label",
    printerOk && uploadsOk
      ? "System healthy"
      : "Needs attention — see tooltip"
  );
  uceHealthStrip.classList.remove("uce-health--warn", "uce-health--bad");
  if (!printerOk) {
    uceHealthStrip.classList.add("uce-health--bad");
  } else if (!uploadsOk) {
    uceHealthStrip.classList.add("uce-health--warn");
  }
  syncHardFailureUi(printerOk, uploadsOk);
}

async function selfHealPrinter() {
  try {
    await refreshPrinterHealthFromBackend();
    if (healthPrinterExact) {
      await updateUceHealthStrip();
      return;
    }
    const now = Date.now();
    if (now - lastPrinterRepairAttemptAt < PRINTER_REPAIR_COOLDOWN_MS) {
      console.warn("[UCE] Printer missing — repair on cooldown");
      await updateUceHealthStrip();
      return;
    }
    lastPrinterRepairAttemptAt = now;
    console.warn("[UCE] Printer missing — attempting repair (silent PDF installer + rename)");
    const rep = await invoke("repair_printer");
    if (rep?.ok) {
      console.info("[UCE] Printer repair:", rep.message);
      logEvent("printer_repaired", rep.message || "success");
    } else {
      console.warn("[UCE] Printer repair:", rep?.message);
      logEvent("printer_repair_failed", rep?.message || "unknown");
    }
    await refreshPrinterHealthFromBackend();
  } catch (e) {
    console.error("[UCE] selfHealPrinter:", e);
  }
  await updateUceHealthStrip();
}

/** Toast + one-click print to FileWisely Printer (Rust `uce-office-print-prompt` / `uce-filewisely-send-doc-prompt`). */
function showOfficeSendToFilewiselyPrompt(path, payload) {
  if (!path || typeof path !== "string") return;
  const message =
    payload &&
    typeof payload.message === "string" &&
    payload.message.trim().length > 0
      ? payload.message.trim()
      : "Send this document to FileWisely?";
  console.info(`[UCE] OFFICE_ROUTING_PROMPT_UI path=${path}`);
  showToast(message, "info", 0);
  const row = document.createElement("div");
  row.style.marginTop = "10px";
  const btn = document.createElement("button");
  btn.type = "button";
  btn.textContent = "Print to FileWisely";
  btn.style.marginRight = "8px";
  btn.style.cursor = "pointer";
  btn.addEventListener("click", async () => {
    try {
      await invoke("uce_office_print_to_filewisely", { path });
      dismissToast();
      showToast("Sent to FileWisely Printer.", "success");
    } catch (err) {
      const msg =
        typeof err === "string" ? err : err?.message || String(err);
      showToast(msg, "error");
    }
  });
  row.appendChild(btn);
  toastEl.appendChild(row);
  void fitWindowToToast();
}

async function uceRuntimePrinterCheck() {
  try {
    const r = await invoke("uce_check_filewisely_printer");
    healthPrinterExact = !!r?.filewisely_exact;
    lastPrinterHealthFetch = Date.now();
    if (!r?.filewisely_exact) {
      console.warn(
        "[UCE] Printer 'FileWisely Printer' not found — run FileWisely installer or rename your PDF printer. Detected:",
        r?.matching_names ?? []
      );
    }
  } catch (e) {
    console.warn("[UCE] uce_check_filewisely_printer:", e);
  }
}

(async function bootstrapUce() {
  try {
    await initTenantContext();
    if (!getBusinessId()) {
      await showTenantSetupDialog();
    }
    await setCompactWindowSize();
  } catch (err) {
    console.error("Initial window layout error:", err);
  }
  setUiState("quiet");
  document.addEventListener("pointermove", (e) => {
    recordUserActivity();
    lastPointerClientX = e.clientX;
    lastPointerClientY = e.clientY;
    updateDockHoverOpacity(e.clientX, e.clientY);
  });
  document.addEventListener(
    "keydown",
    () => {
      recordUserActivity();
    },
    true
  );
  await uceRuntimePrinterCheck();
  await refreshContextState();
  try {
    await listen("uce-incoming-file", (event) => {
      let path = null;
      try {
        const p = event?.payload;
        if (typeof p === "string") path = p;
        else if (p && typeof p.path === "string") path = p.path;
      } catch (_) {
        /* ignore */
      }
      if (path && isFwPdfPath(path)) {
        fwPathsSeenFromIncoming.add(fwKeyPath(path));
        recordFwPdfIncomingForContext();
        console.info("[UCE][JS] incoming received: " + path);
        console.info(`[UCE] trace js_incoming_event_received path=${path}`);
        pulseCaptureCueOnce();
      }
      console.info("[UCE] uce-incoming-file event — scheduling debounced batch");
      scheduleIncomingAutoPdfUpload();
    });
  } catch (e) {
    console.warn("[UCE] uce-incoming-file listener:", e);
  }
  try {
    await listen("uce-office-print-prompt", (event) => {
      recordOfficePrintPromptForContext();
      const pl = event?.payload;
      showOfficeSendToFilewiselyPrompt(pl?.path, pl);
    });
  } catch (e) {
    console.warn("[UCE] uce-office-print-prompt listener:", e);
  }
  try {
    await listen("uce-filewisely-send-doc-prompt", (event) => {
      recordOfficePrintPromptForContext();
      const pl = event?.payload;
      showOfficeSendToFilewiselyPrompt(pl?.path, pl);
    });
  } catch (e) {
    console.warn("[UCE] uce-filewisely-send-doc-prompt listener:", e);
  }
  contextPollTimer = setInterval(refreshContextState, CONTEXT_POLL_MS);
  autoPdfTimer = setInterval(() => void checkAutoPdfUpload("poll"), AUTO_PDF_POLL_MS);
  setInterval(() => void selfHealPrinter(), SELF_HEAL_PRINTER_MS);
  setInterval(
    () => void checkAutoPdfUpload("backup"),
    INCOMING_UPLOAD_BACKUP_MS
  );
  setInterval(() => void fwRescueUploaderTick(), FW_RESCUE_SCAN_MS);
  setInterval(() => void logFwParitySummary(), FW_PARITY_LOG_MS);
  if (WATCH_POLICY_URL && getBusinessId()) {
    void syncWatchPolicyFromRemote();
    setInterval(syncWatchPolicyFromRemote, WATCH_POLICY_POLL_MS);
  }
  roStatusBackgroundTimer = setInterval(() => {
    const url = getRoStatusUrl();
    if (!url || !getBusinessId()) return;
    const { ro, windowTitleForApi } = resolveRoForAwareness();
    if (!ro) return;
    void (async () => {
      try {
        const data = await fetchRoStatus(ro, true, windowTitleForApi);
        if (data) {
          applyRoLevelToButton(data.completeness_level);
        }
      } catch (e) {
        console.error("[UCE] RO status background poll:", e);
      }
    })();
  }, RO_STATUS_BACKGROUND_POLL_MS);
})();

// Dev helper: call window.__uceDebugState() in DevTools.
window.__uceDebugState = async () => invoke("get_debug_state");
window.__uceDetectContext = () => {
  const raw = detectUceContext(lastPolledContext, getUceRecognitionSignals());
  const signals = getUceRecognitionSignals();
  return {
    raw,
    ...stepUceOperatorState(raw, Date.now(), signals),
    operatorDebug: getUceOperatorDebugSnapshot(),
  };
};
window.__uceLastDetectedContext = () => getLastUceDetectedContext();
window.__uceLastOpportunity = () => getLastUceOpportunity();
window.__uceDecisionReasons = () => [...lastUceDecisionReasons];
window.__uceFindSequenceGaps = findSequenceGaps;
window.__uceBuildRoSupplementTruthModel = buildRoSupplementTruthModel;
window.__uceTrustScore = (ro) =>
  getSystemConfidencePercentForRo(
    ro != null && String(ro).trim() ? String(ro).trim() : getMonitoredCurrentRo()
  );
window.__ucePendingMissingSuppress = () => ({
  active: isPendingMissingSuppressActive(),
  untilMs: getSuppressAggressiveMissingUntilMs(),
});
window.__uceUploadDebug = () => lastUploadDebug;
window.__uceTrainButtonVisible = () => getTrainButtonVisible();
window.__uceSetTrainButtonVisible = (v) => setTrainButtonVisible(!!v);
window.__uceSyncWatchPolicy = syncWatchPolicyFromRemote;
window.__uceGetBusinessId = getBusinessId;
window.__uceSaveBusinessId = async (id) => {
  await invoke("save_tenant_business_id", { business_id: String(id).trim() });
  await initTenantContext();
};
window.__uceGetPdfWatchConfig = () => invoke("get_pdf_watch_config");
window.__uceSavePdfWatchConfig = (config) =>
  invoke("save_pdf_watch_config", { config });
window.__uceGetRoStatusUrl = getRoStatusUrl;
window.__uceFetchRoStatus = fetchRoStatus;
window.__uceExtractRoFromTitle = extractRoFromTitleForMonitor;
window.__uceIsCccWorkflowWindow = isCccWorkflowWindow;
window.__uceResolveRoForAwareness = resolveRoForAwareness;
window.__uceHealthStrip = updateUceHealthStrip;
window.__uceSelfHealPrinter = selfHealPrinter;
window.__uceGetEventLog = getUceEventLog;
window.__uceLogEvent = logEvent;
/** Installed app version from `tauri.conf.json` / bundle (compare to GitHub release). */
window.__uceAppVersion = () => getVersion();
/** Force an update check now (ignores 15‑min throttle). Returns a small status object. */
window.__uceCheckUpdate = () => runUceAppUpdateCheck({ force: true });