/**
 * Lightweight session persistence for operator trust + Theo coordination.
 */

const LS_KEY = "uce_trust_ro_v1";
const PENDING_CAPTURE_SUPPRESS_MS = 3000;

let suppressAggressiveMissingUntil = 0;

export function getPendingCaptureSuppressMs() {
  return PENDING_CAPTURE_SUPPRESS_MS;
}

export function armPendingMissingSuppressBuffer(now = Date.now()) {
  suppressAggressiveMissingUntil = now + PENDING_CAPTURE_SUPPRESS_MS;
}

/** @param {number} [now] */
export function isPendingMissingSuppressActive(now = Date.now()) {
  return now < suppressAggressiveMissingUntil;
}

/** @returns {number} epoch ms until aggressive “missing” messaging should be softened */
export function getSuppressAggressiveMissingUntilMs() {
  return suppressAggressiveMissingUntil;
}

function loadMap() {
  try {
    const raw = localStorage.getItem(LS_KEY);
    if (!raw) return {};
    const o = JSON.parse(raw);
    return o && typeof o === "object" ? o : {};
  } catch {
    return {};
  }
}

function saveMap(m) {
  try {
    localStorage.setItem(LS_KEY, JSON.stringify(m));
  } catch {
    /* ignore */
  }
}

/** Call when `uce-context-detected` actually fires (known context). */
export function noteUceContextDetectionEmitted(roId) {
  if (!roId || !/^\d{4,6}$/.test(String(roId))) return;
  const id = String(roId);
  const m = loadMap();
  m[id] = m[id] || { detections: 0, captures: 0 };
  m[id].detections = (m[id].detections || 0) + 1;
  m[id].lastDetectionAt = Date.now();
  saveMap(m);
}

/** Call after a successful upload while context is relevant. */
export function noteUceContextCaptureSuccess(roId) {
  if (!roId || !/^\d{4,6}$/.test(String(roId))) return;
  const id = String(roId);
  const m = loadMap();
  m[id] = m[id] || { detections: 0, captures: 0 };
  m[id].captures = (m[id].captures || 0) + 1;
  m[id].lastCaptureAt = Date.now();
  saveMap(m);
}

/**
 * Simple reliability: captures per detection event (not per-doc accounting).
 * @param {string} [roId]
 * @returns {number | null} 0–100 or null if no data
 */
export function getSystemConfidencePercentForRo(roId) {
  if (!roId) return null;
  const id = String(roId);
  const m = loadMap()[id];
  if (!m) return null;
  const d = Math.max(0, Number(m.detections) || 0);
  const c = Math.max(0, Number(m.captures) || 0);
  if (d === 0 && c === 0) return null;
  if (d === 0) return 100;
  const pct = Math.round((100 * Math.min(c, d)) / d);
  return Math.min(100, Math.max(0, pct));
}
