/**
 * Human-facing “why” strings for hover copy, Theo payloads, and debug.
 * Pure functions — no DOM.
 */

/** @typedef {import('./uceContextRecognition.js').UceDetectedContext} UceDetectedContext */

function typeHeadline(type, bucket) {
  const human = {
    ccc_estimate: "CCC Estimate",
    ccc_supplement: "CCC Supplement",
    ccc_final_bill: "CCC Final Bill",
    ccc_print_dialog: "CCC Print / workfile",
    tesla_epc: "Tesla EPC",
    parts_invoice: "Parts / invoice",
    unknown: "Unknown context",
  };
  const h = human[type] || type.replace(/_/g, " ");
  if (bucket === "candidate") return `Possible: ${h}`;
  if (bucket === "known") return `Detected: ${h}`;
  return h;
}

function confidenceLabel(bucket, conf) {
  if (bucket === "unknown") return "Low — no strong match";
  if (bucket === "candidate") return "Building confidence";
  if (conf >= 0.95) return "High";
  if (conf >= 0.85) return "Strong";
  return "Moderate";
}

/**
 * @param {{
 *   detected: UceDetectedContext | null | undefined,
 *   opportunity: { type: string, context: string },
 *   rawDetected: UceDetectedContext | null | undefined,
 *   contextEnteredAt: number,
 *   signals: { msSinceFwPdfIncoming?: number, msSinceOfficePrintPrompt?: number, msSinceUserActivity?: number },
 *   now: number,
 *   windowTitle?: string,
 * }} p
 */
export function buildUceReasonPayload(p) {
  const {
    detected,
    opportunity,
    rawDetected,
    contextEnteredAt,
    signals,
    now,
    windowTitle,
  } = p;

  /** @type {string[]} */
  const decisionReasons = [];
  /** @type {string[]} */
  const bullets = [];

  const title = String(windowTitle || "").toLowerCase();
  const msStable = detected && contextEnteredAt ? now - contextEnteredAt : 0;
  const secStable = (msStable / 1000).toFixed(1);

  const msFw = signals.msSinceFwPdfIncoming ?? Number.POSITIVE_INFINITY;
  const msPrint = signals.msSinceOfficePrintPrompt ?? Number.POSITIVE_INFINITY;
  const msUser = signals.msSinceUserActivity ?? 0;

  if (msStable > 400 && detected?.bucket === "known") {
    bullets.push(`Context stable ~${secStable}s for this classification`);
    decisionReasons.push(`stable_for_${Math.round(msStable)}ms`);
  }

  if (msFw < 1500) {
    bullets.push("Recent PDF write in watched Incoming (≤1.5s)");
    decisionReasons.push("pdf_recent_incoming");
  }

  if (msPrint < 3500) {
    bullets.push("Office / FileWisely print flow active recently");
    decisionReasons.push("print_prompt_recent");
  }

  if (/\b(print|printing|print\s*preview|workfile)\b/.test(title)) {
    bullets.push("Window title mentions print or workfile");
    decisionReasons.push("title_print_signals");
  }

  if (/\bccc\b|ccc one|cccone|repair\s*order|\bro[#\s-]?\d/.test(title)) {
    bullets.push("Title matches CCC / repair-order style pattern");
    decisionReasons.push("title_ccc_pattern");
  }

  if (rawDetected && detected && rawDetected.confidence < detected.confidence) {
    bullets.push("Confidence strengthened after sustained same context (time-based)");
    decisionReasons.push("time_escalation");
  }

  if (detected?.bucket === "candidate") {
    bullets.push("Waiting for stronger or longer-lived signals before “known”");
    decisionReasons.push("bucket_candidate");
  }

  if (opportunity.type === "ready_to_capture") {
    bullets.push("Capture window: print and/or PDF activity line up");
    decisionReasons.push("opportunity_ready");
  } else if (opportunity.type === "likely_capture") {
    bullets.push(`Idle ≥0.8s — suggesting you may want to capture (${typeHeadline(detected?.type || "unknown", "known")})`);
    decisionReasons.push("opportunity_likely_idle");
  }

  if (msUser >= 800 && detected?.bucket === "known") {
    bullets.push("Brief pause in mouse / keys (less navigation noise)");
    decisionReasons.push("user_pause_ok");
  }

  if (bullets.length === 0) {
    bullets.push("Watching foreground window and FileWisely signals");
    decisionReasons.push("baseline_poll");
  }

  const headline =
    opportunity.type === "ready_to_capture"
      ? "Ready to capture"
      : typeHeadline(detected?.type || "unknown", detected?.bucket || "unknown");

  const confLabel = confidenceLabel(
    detected?.bucket || "unknown",
    detected?.confidence ?? 0
  );

  /** @type {'waiting_for_capture' | 'seen_in_ccc' | 'neutral'} */
  let suggestedTheoTone = "neutral";
  if (detected?.bucket === "known" && detected.type.startsWith("ccc_")) {
    suggestedTheoTone = "waiting_for_capture";
  }
  if (detected?.bucket === "candidate" && String(detected.type).includes("supplement")) {
    suggestedTheoTone = "seen_in_ccc";
  }

  const tooltipLines = [
    headline,
    `Confidence: ${confLabel}`,
    "",
    "Reason:",
    ...bullets.map((b) => `• ${b}`),
  ];

  return {
    headline,
    confidenceLabel: confLabel,
    bullets,
    decisionReasons,
    suggestedTheoTone,
    tooltipText: tooltipLines.join("\n"),
    shortSummary: `${headline} · ${confLabel}`,
  };
}
