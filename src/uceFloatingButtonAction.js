/**
 * Action layer: applies recognition output to the floating capture control (tooltip, chrome, capture-mode hint).
 * Does not perform detection — use `detectUceContext` for that.
 */

/** @typedef {import('./uceContextRecognition.js').UceDetectedContext} UceDetectedContext */
/** @typedef {import('./uceContextOperatorState.js').UceOpportunity} UceOpportunity */

const BASE_TITLE = "Capture current screen or document";

const GLOW_CLASSES = [
  "uce-ctx-glow--estimate",
  "uce-ctx-glow--supplement",
  "uce-ctx-glow--final_bill",
  "uce-ctx-glow--print",
  "uce-ctx-glow--tesla",
  "uce-ctx-glow--parts",
];

const TYPE_CLASSES = [
  "uce-ctx--ccc_estimate",
  "uce-ctx--ccc_supplement",
  "uce-ctx--ccc_final_bill",
  "uce-ctx--ccc_print_dialog",
  "uce-ctx--tesla_epc",
  "uce-ctx--parts_invoice",
  "uce-ctx--unknown",
];

/** @param {UceDetectedContext | null | undefined} detected */
function tooltipFor(detected) {
  if (!detected || detected.bucket === "unknown") return BASE_TITLE;
  if (detected.bucket === "candidate") {
    switch (detected.type) {
      case "ccc_estimate":
        return "Possible estimate — click if this matches what you see";
      case "ccc_supplement":
        return "Possible supplement — click if this matches what you see";
      case "ccc_final_bill":
        return "Possible Final Bill — click if this matches what you see";
      case "ccc_print_dialog":
        return "Print may be active — not certain yet";
      case "tesla_epc":
        return "Possible Tesla EPC — click to capture";
      case "parts_invoice":
        return "Possible parts / invoice — click to capture";
      default:
        return BASE_TITLE;
    }
  }
  switch (detected.type) {
    case "ccc_estimate":
      return "Detected: Estimate — click to capture (PDF)";
    case "ccc_supplement":
      return "Detected: Supplement — click to capture (PDF)";
    case "ccc_final_bill":
      return "Detected: Final Bill — click to capture (PDF)";
    case "ccc_print_dialog":
      return "Print detected — click to capture (PDF)";
    case "tesla_epc":
      return "Tesla EPC — click to capture";
    case "parts_invoice":
      return "Parts / invoice — click to capture";
    default:
      return BASE_TITLE;
  }
}

/** @type {UceDetectedContext | null} */
let lastAppliedDetection = null;
/** @type {'ready_to_capture' | 'likely_capture' | 'idle'} */
let lastAppliedOpportunityKind = "idle";

const READY_PULSE_COOLDOWN_MS = 4000;
let lastReadyPulseAt = 0;

/**
 * When bucket is `known`, use detection for preferred capture mode; otherwise caller should fall back.
 * @param {UceDetectedContext | null | undefined} detected
 * @returns {'pdf' | 'screenshot' | null}
 */
export function preferredCaptureModeFromUceDetection(detected) {
  if (!detected || detected.bucket !== "known") return null;
  return detected.preferredCaptureMode === "pdf" ? "pdf" : "screenshot";
}

/** @returns {UceDetectedContext | null} */
export function getLastUceDetectedContext() {
  return lastAppliedDetection;
}

/**
 * @param {HTMLElement | null} el
 * @param {'pdf' | 'screenshot'} mode
 */
export function applyCaptureModeBadge(el, mode) {
  if (!el) return;
  el.textContent = mode === "pdf" ? "\u{1F4C4}" : "\u{1F5BC}\u{FE0F}";
  el.title = mode === "pdf" ? "Next capture: PDF path" : "Next capture: Screenshot";
  el.hidden = false;
}

/**
 * @param {HTMLButtonElement | null} button
 * @param {UceDetectedContext | null | undefined} detected
 * @param {{
 *   pulseOnce?: () => void,
 *   opportunity?: UceOpportunity | null,
 *   resolvedCaptureMode?: 'pdf' | 'screenshot',
 *   modeBadgeEl?: HTMLElement | null,
 *   now?: number,
 *   reason?: { tooltipText?: string, decisionReasons?: string[] } | null,
 * }} [opts]
 */
export function applyUceFloatingButtonChrome(button, detected, opts = {}) {
  if (!button) return;
  lastAppliedDetection = detected || null;

  const now =
    typeof opts.now === "number" && Number.isFinite(opts.now)
      ? opts.now
      : Date.now();

  const opportunity = opts.opportunity || { type: "idle", context: "unknown" };
  const oppKind = opportunity.type;

  for (const c of GLOW_CLASSES) button.classList.remove(c);
  for (const c of TYPE_CLASSES) button.classList.remove(c);
  button.classList.remove(
    "uce-btn--context-glow",
    "uce-btn--opportunity-likely",
    "uce-btn--candidate-uncertainty",
    "uce-ctx-glow--candidate-soft",
    "uce-glow-strength--low",
    "uce-glow-strength--mid",
    "uce-glow-strength--high"
  );

  const d = detected;
  const reason = opts.reason;
  /** One-line hover; avoid multi-line `tooltipText` (felt like a large “tile”). Full reasons stay in `data-uce-decision-reasons` for debug. */
  const short =
    reason?.shortSummary && String(reason.shortSummary).trim()
      ? String(reason.shortSummary).trim()
      : "";
  button.setAttribute("title", short || tooltipFor(d));
  if (reason?.decisionReasons?.length) {
    try {
      button.setAttribute(
        "data-uce-decision-reasons",
        JSON.stringify(reason.decisionReasons)
      );
    } catch {
      button.removeAttribute("data-uce-decision-reasons");
    }
  } else {
    button.removeAttribute("data-uce-decision-reasons");
  }
  button.classList.add(`uce-ctx--${d?.type || "unknown"}`);

  if (typeof opts.resolvedCaptureMode === "string") {
    applyCaptureModeBadge(
      opts.modeBadgeEl ?? null,
      opts.resolvedCaptureMode === "pdf" ? "pdf" : "screenshot"
    );
  } else if (opts.modeBadgeEl) {
    opts.modeBadgeEl.hidden = true;
  }

  if (
    oppKind === "ready_to_capture" &&
    lastAppliedOpportunityKind !== "ready_to_capture" &&
    typeof opts.pulseOnce === "function" &&
    now - lastReadyPulseAt >= READY_PULSE_COOLDOWN_MS
  ) {
    opts.pulseOnce();
    lastReadyPulseAt = now;
  }
  lastAppliedOpportunityKind = oppKind;

  if (!d) {
    return;
  }

  if (d.bucket === "candidate") {
    button.classList.add(
      "uce-btn--candidate-uncertainty",
      "uce-ctx-glow--candidate-soft"
    );
    return;
  }

  if (d.bucket !== "known") {
    return;
  }

  if (oppKind === "likely_capture") {
    button.classList.add("uce-btn--opportunity-likely");
  }

  switch (d.type) {
    case "ccc_estimate":
      button.classList.add("uce-ctx-glow--estimate", "uce-btn--context-glow");
      break;
    case "ccc_supplement":
      button.classList.add("uce-ctx-glow--supplement", "uce-btn--context-glow");
      break;
    case "ccc_final_bill":
      button.classList.add("uce-ctx-glow--final_bill", "uce-btn--context-glow");
      break;
    case "ccc_print_dialog":
      button.classList.add("uce-ctx-glow--print", "uce-btn--context-glow");
      break;
    case "tesla_epc":
      button.classList.add("uce-ctx-glow--tesla", "uce-btn--context-glow");
      break;
    case "parts_invoice":
      button.classList.add("uce-ctx-glow--parts", "uce-btn--context-glow");
      break;
    default:
      break;
  }

  if (button.classList.contains("uce-btn--context-glow")) {
    const conf = typeof d.confidence === "number" ? d.confidence : 0;
    if (conf >= 0.97) button.classList.add("uce-glow-strength--high");
    else if (conf >= 0.9) button.classList.add("uce-glow-strength--mid");
    else button.classList.add("uce-glow-strength--low");
  }
}
