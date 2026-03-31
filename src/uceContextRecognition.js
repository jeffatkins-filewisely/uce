/**
 * UCE context recognition v1 — classification only (no DOM).
 * Consumes `get_watch_context` payload + optional local signals; outputs a ranked detection.
 */

import { inferCccDocSignalsFromTitle } from "./uceCccTitleSignals.js";

/** @typedef {'ccc_estimate' | 'ccc_supplement' | 'ccc_final_bill' | 'ccc_print_dialog' | 'tesla_epc' | 'parts_invoice' | 'unknown'} UceContextType */

/** @typedef {'known' | 'candidate' | 'unknown'} UceContextBucket */

/**
 * @typedef {Object} UceDetectedContext
 * @property {UceContextType} type
 * @property {UceContextBucket} bucket
 * @property {number} confidence
 * @property {string} [sourceApp]
 * @property {string} [windowTitle]
 * @property {string} [matchedRule]
 * @property {string} timestamp ISO-8601
 * @property {'pdf' | 'screenshot'} preferredCaptureMode
 */

const TYPE_PRIORITY = {
  ccc_print_dialog: 100,
  ccc_final_bill: 90,
  ccc_supplement: 80,
  ccc_estimate: 70,
  tesla_epc: 60,
  parts_invoice: 50,
  unknown: 0,
};

/** @param {unknown} b */
function normalizeRustBucket(b) {
  const s = String(b || "").toLowerCase();
  if (s === "known" || s === "candidate" || s === "unknown") return s;
  return "unknown";
}

function looksCccSurfaces(app, title) {
  const a = app.toLowerCase();
  const t = title.toLowerCase();
  return (
    t.includes("ccc") ||
    t.includes("ccc one") ||
    t.includes("cccone") ||
    a.includes("ccc") ||
    /^\d{4,6}\b/.test(t.trim()) ||
    /\b(ro[#\s-]|repair\s*order)\b/i.test(t) ||
    /\bro\d{4,6}\b/i.test(t)
  );
}

/**
 * @param {string} ruleId
 * @returns {UceContextType}
 */
function mapRustRuleToType(ruleId) {
  const id = ruleId.toLowerCase();
  if (id === "ccc_estimate") return "ccc_estimate";
  if (id === "ccc_supplement") return "ccc_supplement";
  if (id === "ccc_final_bill") return "ccc_final_bill";
  if (id.startsWith("tesla_epc")) return "tesla_epc";
  if (id.startsWith("partstrader") || id.startsWith("parts_trader"))
    return "parts_invoice";
  if (id.startsWith("ccc_trained") || id === "ccc_open") return "unknown";
  return "unknown";
}

/**
 * @param {UceContextType} type
 * @returns {'pdf' | 'screenshot'}
 */
function preferredModeForType(type) {
  if (
    type === "ccc_estimate" ||
    type === "ccc_supplement" ||
    type === "ccc_final_bill" ||
    type === "ccc_print_dialog"
  ) {
    return "pdf";
  }
  return "screenshot";
}

/**
 * @param {Record<string, unknown> | null | undefined} watchContext from `get_watch_context`
 * @param {Partial<{ msSinceFwPdfIncoming: number, msSinceOfficePrintPrompt: number }>} [signals]
 * @returns {UceDetectedContext}
 */
export function detectUceContext(watchContext, signals = {}) {
  const ts = new Date().toISOString();
  const app = String(watchContext?.source_app ?? "").trim();
  const title = String(watchContext?.window_title ?? "").trim();
  const t = title.toLowerCase();
  const rule = String(watchContext?.matched_rule ?? "none").trim();
  const ruleL = rule.toLowerCase();
  const wf = String(watchContext?.workflow_kind ?? "").toLowerCase();
  const rustBucket = normalizeRustBucket(watchContext?.bucket);
  const actionAllowed = watchContext?.action_allowed === true;

  const msFw = signals.msSinceFwPdfIncoming ?? Number.POSITIVE_INFINITY;
  const msPrintPrompt =
    signals.msSinceOfficePrintPrompt ?? Number.POSITIVE_INFINITY;
  const fwRecent = msFw < 12_000;
  const printPromptRecent = msPrintPrompt < 8_000;

  const cccSurface = looksCccSurfaces(app, title);
  const docSignals = inferCccDocSignalsFromTitle(title);
  const hasPrintSignal = docSignals.some((s) => s.key === "print_panel");
  const hasFinal = docSignals.some((s) => s.key === "final_bill");
  const hasSupp = docSignals.some((s) => String(s.key).startsWith("supplement_"));
  const hasEst = docSignals.some((s) => s.key === "estimate");
  const workfilePrint =
    /\bworkfile\b/i.test(t) && /\bprint/i.test(t);

  const printish =
    hasPrintSignal ||
    workfilePrint ||
    /\bprint\s*to\s*file\b/i.test(t) ||
    /\bworkfile\s*print\b/i.test(t);

  /** @type {{ type: UceContextType, confidence: number, bucket: UceContextBucket, matchedRule: string }[]} */
  const hits = [];

  if (
    printish &&
    (cccSurface ||
      wf === "ccc" ||
      ruleL.startsWith("ccc_") ||
      ruleL.startsWith("ccc_trained"))
  ) {
    const strongTitle = cccSurface || wf === "ccc" || ruleL.startsWith("ccc");
    let conf = strongTitle ? 0.92 : 0.62;
    if (printPromptRecent) conf = Math.max(conf, 0.88);
    if (fwRecent && strongTitle) conf = Math.max(conf, 0.78);
    let b = "candidate";
    if (strongTitle || printPromptRecent || rustBucket === "known") b = "known";
    hits.push({
      type: "ccc_print_dialog",
      confidence: Math.min(0.97, conf),
      bucket: /** @type {UceContextBucket} */ (b),
      matchedRule: "js_ccc_print_dialog",
    });
  }

  if (
    hasFinal ||
    (/\binvoice\b/i.test(t) &&
      cccSurface &&
      !/\bpartstrader\b/i.test(t) &&
      !/\bparts\s*trader\b/i.test(t) &&
      !/\bparts\s+invoice\b/i.test(t))
  ) {
    const conf = hasFinal ? 0.9 : 0.72;
    const b =
      hasFinal && cccSurface
        ? "known"
        : rustBucket === "known" && wf === "ccc"
          ? "known"
          : "candidate";
    hits.push({
      type: "ccc_final_bill",
      confidence: conf,
      bucket: /** @type {UceContextBucket} */ (b),
      matchedRule: "js_ccc_final_bill",
    });
  }

  if (
    hasSupp ||
    /\bsupplement\b/i.test(t) ||
    /\bsupp\.?\s*#?\s*\d/i.test(t) ||
    /\baddendum\b/i.test(t)
  ) {
    if (!/\bestimate\b/i.test(t) || hasSupp || /\bsupplement\b/i.test(t)) {
      const conf = hasSupp && cccSurface ? 0.91 : cccSurface ? 0.78 : 0.55;
      const b =
        hasSupp && cccSurface
          ? "known"
          : rustBucket === "known" && wf === "ccc"
            ? "known"
            : "candidate";
      hits.push({
        type: "ccc_supplement",
        confidence: conf,
        bucket: /** @type {UceContextBucket} */ (b),
        matchedRule: "js_ccc_supplement",
      });
    }
  }

  if (
    hasEst ||
    (/\bestimate\b/i.test(t) &&
      /\b(appraisal|repair\s*plan|initial)\b/i.test(t)) ||
    (/\bestimate\b/i.test(t) && cccSurface && !hasSupp && !hasPrintSignal)
  ) {
    const conf =
      hasEst && cccSurface ? 0.9 : /\b(appraisal|repair\s*plan)\b/i.test(t) ? 0.82 : 0.68;
    const b =
      hasEst && cccSurface
        ? "known"
        : rustBucket === "known" && wf === "ccc"
          ? "known"
          : "candidate";
    hits.push({
      type: "ccc_estimate",
      confidence: conf,
      bucket: /** @type {UceContextBucket} */ (b),
      matchedRule: "js_ccc_estimate",
    });
  }

  if (
    /\btesla\b/i.test(t) &&
    /\b(epc|parts\s*catalog|electronic\s*parts)\b/i.test(t)
  ) {
    hits.push({
      type: "tesla_epc",
      confidence: rustBucket === "known" ? 0.9 : 0.72,
      bucket: rustBucket === "known" ? "known" : "candidate",
      matchedRule: "js_tesla_epc",
    });
  }

  if (
    /\bpartstrader\b/i.test(t) ||
    /\bparts\s*trader\b/i.test(t) ||
    (/\binvoice\b/i.test(t) &&
      /\b(parts|vendor|supplier|po[#\s-]?)\b/i.test(t))
  ) {
    hits.push({
      type: "parts_invoice",
      confidence: rustBucket === "known" ? 0.88 : 0.65,
      bucket: rustBucket === "known" ? "known" : "candidate",
      matchedRule: "js_parts_invoice",
    });
  }

  const mapped = mapRustRuleToType(ruleL);
  if (mapped !== "unknown" && rustBucket === "known") {
    hits.push({
      type: mapped,
      confidence: actionAllowed ? 0.86 : 0.8,
      bucket: "known",
      matchedRule: rule,
    });
  }

  if (hits.length === 0) {
    return {
      type: "unknown",
      bucket: rustBucket === "unknown" ? "unknown" : "candidate",
      confidence: 0,
      sourceApp: app,
      windowTitle: title,
      matchedRule: rule,
      timestamp: ts,
      preferredCaptureMode: "screenshot",
    };
  }

  hits.sort((x, y) => {
    if (Math.abs(y.confidence - x.confidence) > 0.001) {
      return y.confidence - x.confidence;
    }
    return (TYPE_PRIORITY[y.type] || 0) - (TYPE_PRIORITY[x.type] || 0);
  });

  const best = hits[0];

  return {
    type: best.type,
    bucket: best.bucket,
    confidence: best.confidence,
    sourceApp: app,
    windowTitle: title,
    matchedRule: best.matchedRule || rule,
    timestamp: ts,
    preferredCaptureMode: preferredModeForType(best.type),
  };
}
