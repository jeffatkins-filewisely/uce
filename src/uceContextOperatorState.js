/**
 * Operator confidence layer: stability memory, time-based escalation, downgrade smoothing,
 * CCC doc-group holds, confidence drop penalty, and opportunity timing.
 */

/** @typedef {import('./uceContextRecognition.js').UceDetectedContext} UceDetectedContext */
/** @typedef {import('./uceContextRecognition.js').UceContextType} UceContextType */

/**
 * @typedef {'ready_to_capture' | 'likely_capture' | 'idle'} UceOpportunityKind
 */

/**
 * @typedef {Object} UceOpportunity
 * @property {UceOpportunityKind} type
 * @property {UceContextType} context
 */

const MIN_HOLD_KNOWN_MS = 2000;
/** Shorter hold when switching between CCC document contexts (estimate ↔ supplement ↔ …). */
const MIN_HOLD_CCC_DOC_GROUP_MS = 500;
const DOWNGRADE_GRACE_MS = 1500;
const USER_PAUSE_MS = 800;
/** +0.05 confidence per this many ms of stable same `type` (machine-independent). */
const ESCALATION_INTERVAL_MS = 250;
const ESCALATION_STEP = 0.05;
const CANDIDATE_PROMOTE_THRESHOLD = 0.88;
const KNOWN_CONFIDENCE_FLOOR = 0.9;
/** Raw confidence drop vs previous tick before applying penalty. */
const CONFIDENCE_DROP_THRESHOLD = 0.15;
const CONFIDENCE_DROP_MULTIPLIER = 0.7;

/** @type {UceDetectedContext | null} */
let lastStableOutput = null;
let stableContextEnteredAt = 0;
/** Hold duration chosen when `stableContextEnteredAt` was last reset (for debug). */
let lastCommittedHoldMs = MIN_HOLD_KNOWN_MS;

let escalationAnchorType = "";
let escalationAnchorAt = 0;

let prevRecognizerConfidence = null;

/** @type {UceOpportunity | null} */
let lastOpportunity = { type: "idle", context: "unknown" };

let downgradeGraceStartedAt = 0;
/** @type {UceDetectedContext | null} */
let frozenKnownDuringDowngrade = null;

/** @type {UceDetectedContext | null} */
let lastRawDetectedSnapshot = null;
/** @type {UceDetectedContext | null} */
let lastProposedSnapshot = null;

/**
 * @param {UceContextType | string} type
 * @returns {string}
 */
function contextGroup(type) {
  const t = String(type || "");
  if (
    t === "ccc_estimate" ||
    t === "ccc_supplement" ||
    t === "ccc_final_bill" ||
    t === "ccc_print_dialog"
  ) {
    return "ccc_doc";
  }
  return `other:${t}`;
}

/**
 * @param {UceDetectedContext | null | undefined} lastStable
 * @param {UceDetectedContext} proposed
 */
function effectiveKnownHoldMs(lastStable, proposed) {
  if (
    lastStable &&
    lastStable.bucket === "known" &&
    contextGroup(lastStable.type) === "ccc_doc" &&
    contextGroup(proposed.type) === "ccc_doc"
  ) {
    return MIN_HOLD_CCC_DOC_GROUP_MS;
  }
  return MIN_HOLD_KNOWN_MS;
}

/**
 * @param {UceDetectedContext | null | undefined} d
 */
function stableIdentityKey(d) {
  if (!d) return "";
  return `${d.type}|${d.bucket}`;
}

/**
 * @param {UceDetectedContext} raw
 * @returns {UceDetectedContext}
 */
function applyConfidenceDropPenalty(raw) {
  let c = raw.confidence;
  if (
    prevRecognizerConfidence != null &&
    raw.confidence < prevRecognizerConfidence - CONFIDENCE_DROP_THRESHOLD
  ) {
    c = raw.confidence * CONFIDENCE_DROP_MULTIPLIER;
  }
  prevRecognizerConfidence = raw.confidence;
  return { ...raw, confidence: c };
}

/**
 * @param {UceDetectedContext} raw
 * @param {number} now
 * @returns {UceDetectedContext}
 */
function applyEscalation(raw, now) {
  if (raw.type !== escalationAnchorType) {
    escalationAnchorType = raw.type;
    escalationAnchorAt = now;
  }
  const ticks = Math.floor((now - escalationAnchorAt) / ESCALATION_INTERVAL_MS);
  let boosted = Math.min(1, raw.confidence + ticks * ESCALATION_STEP);
  let bucket = raw.bucket;
  if (bucket === "candidate" && boosted >= CANDIDATE_PROMOTE_THRESHOLD) {
    bucket = "known";
  }
  if (bucket === "known") {
    boosted = Math.max(boosted, KNOWN_CONFIDENCE_FLOOR);
  }

  return {
    ...raw,
    confidence: boosted,
    bucket,
  };
}

/**
 * @param {UceDetectedContext | null | undefined} detected
 * @param {number} now
 * @param {number} contextEnteredAt
 * @param {{
 *   msSinceOfficePrintPrompt?: number,
 *   msSinceFwPdfIncoming?: number,
 *   msSinceUserActivity?: number,
 * }} signals
 * @returns {UceOpportunity}
 */
export function computeOpportunity(detected, now, contextEnteredAt, signals) {
  if (!detected || detected.bucket !== "known") {
    return { type: "idle", context: detected?.type ?? "unknown" };
  }

  const msPrintPrompt = signals.msSinceOfficePrintPrompt ?? Number.POSITIVE_INFINITY;
  const printPromptHot = msPrintPrompt < 3500;
  const msFw = signals.msSinceFwPdfIncoming ?? Number.POSITIVE_INFINITY;
  const fwCorrelation = msFw < 1500;

  const title = (detected.windowTitle || "").toLowerCase();
  const titlePrint =
    /\b(print|printing|print\s*preview|workfile)\b/.test(title);

  if (detected.type === "ccc_print_dialog") {
    return { type: "ready_to_capture", context: detected.type };
  }

  if (
    fwCorrelation &&
    (printPromptHot || titlePrint) &&
    (detected.type.startsWith("ccc_") ||
      detected.type === "tesla_epc" ||
      detected.type === "parts_invoice")
  ) {
    return { type: "ready_to_capture", context: detected.type };
  }

  if (
    printPromptHot &&
    (detected.type === "tesla_epc" || detected.type === "parts_invoice")
  ) {
    return { type: "ready_to_capture", context: detected.type };
  }

  const heldMs = now - contextEnteredAt;
  const msSinceUser = signals.msSinceUserActivity ?? 0;
  const userPaused = msSinceUser >= USER_PAUSE_MS;

  if (
    heldMs >= MIN_HOLD_KNOWN_MS &&
    userPaused &&
    (detected.type === "ccc_supplement" ||
      detected.type === "ccc_estimate" ||
      detected.type === "ccc_final_bill")
  ) {
    return { type: "likely_capture", context: detected.type };
  }

  return { type: "idle", context: detected.type };
}

function resetOperatorState() {
  lastStableOutput = null;
  stableContextEnteredAt = 0;
  lastCommittedHoldMs = MIN_HOLD_KNOWN_MS;
  escalationAnchorType = "";
  escalationAnchorAt = 0;
  prevRecognizerConfidence = null;
  lastOpportunity = { type: "idle", context: "unknown" };
  downgradeGraceStartedAt = 0;
  frozenKnownDuringDowngrade = null;
  lastRawDetectedSnapshot = null;
  lastProposedSnapshot = null;
}

/**
 * @param {UceDetectedContext | null | undefined} rawDetected
 * @param {number} now
 * @param {{
 *   msSinceOfficePrintPrompt?: number,
 *   msSinceFwPdfIncoming?: number,
 *   msSinceUserActivity?: number,
 * }} [signals]
 */
export function stepUceOperatorState(rawDetected, now, signals = {}) {
  if (!rawDetected) {
    resetOperatorState();
    return {
      detected: null,
      opportunity: lastOpportunity,
      rawDetected,
      contextEnteredAt: 0,
    };
  }

  const penalized = applyConfidenceDropPenalty(rawDetected);
  lastRawDetectedSnapshot = { ...penalized };
  const proposed = applyEscalation(penalized, now);
  lastProposedSnapshot = { ...proposed };
  const proposedKey = stableIdentityKey(proposed);

  if (proposed.bucket === "known") {
    downgradeGraceStartedAt = 0;
    frozenKnownDuringDowngrade = null;
  }

  if (downgradeGraceStartedAt > 0) {
    const elapsed = now - downgradeGraceStartedAt;
    if (proposed.bucket === "known") {
      downgradeGraceStartedAt = 0;
      frozenKnownDuringDowngrade = null;
    } else if (elapsed >= DOWNGRADE_GRACE_MS) {
      downgradeGraceStartedAt = 0;
      frozenKnownDuringDowngrade = null;
      lastStableOutput = { ...proposed };
      stableContextEnteredAt = now;
      lastCommittedHoldMs = effectiveKnownHoldMs(null, proposed);
      lastOpportunity = computeOpportunity(
        lastStableOutput,
        now,
        stableContextEnteredAt,
        signals
      );
      return {
        detected: { ...lastStableOutput },
        opportunity: lastOpportunity,
        rawDetected,
        contextEnteredAt: stableContextEnteredAt,
      };
    } else if (frozenKnownDuringDowngrade) {
      lastOpportunity = computeOpportunity(
        frozenKnownDuringDowngrade,
        now,
        stableContextEnteredAt,
        signals
      );
      return {
        detected: { ...frozenKnownDuringDowngrade },
        opportunity: lastOpportunity,
        rawDetected,
        contextEnteredAt: stableContextEnteredAt,
      };
    }
  }

  if (
    lastStableOutput?.bucket === "known" &&
    proposed.bucket !== "known" &&
    downgradeGraceStartedAt === 0
  ) {
    downgradeGraceStartedAt = now;
    frozenKnownDuringDowngrade = { ...lastStableOutput };
    lastOpportunity = computeOpportunity(
      frozenKnownDuringDowngrade,
      now,
      stableContextEnteredAt,
      signals
    );
    return {
      detected: { ...frozenKnownDuringDowngrade },
      opportunity: lastOpportunity,
      rawDetected,
      contextEnteredAt: stableContextEnteredAt,
    };
  }

  const holdMs = effectiveKnownHoldMs(lastStableOutput, proposed);
  if (
    lastStableOutput &&
    lastStableOutput.bucket === "known" &&
    now - stableContextEnteredAt < holdMs
  ) {
    const stableKey = stableIdentityKey(lastStableOutput);
    if (
      proposedKey !== stableKey &&
      proposed.confidence <= lastStableOutput.confidence
    ) {
      lastOpportunity = computeOpportunity(
        lastStableOutput,
        now,
        stableContextEnteredAt,
        signals
      );
      return {
        detected: { ...lastStableOutput },
        opportunity: lastOpportunity,
        rawDetected,
        contextEnteredAt: stableContextEnteredAt,
      };
    }
  }

  if (!lastStableOutput || proposedKey !== stableIdentityKey(lastStableOutput)) {
    stableContextEnteredAt = now;
    lastCommittedHoldMs = effectiveKnownHoldMs(lastStableOutput, proposed);
  }

  lastStableOutput = { ...proposed };
  lastOpportunity = computeOpportunity(
    lastStableOutput,
    now,
    stableContextEnteredAt,
    signals
  );

  return {
    detected: { ...lastStableOutput },
    opportunity: lastOpportunity,
    rawDetected,
    contextEnteredAt: stableContextEnteredAt,
  };
}

/** @returns {UceOpportunity} */
export function getLastUceOpportunity() {
  return lastOpportunity || { type: "idle", context: "unknown" };
}

/**
 * @param {number} [now]
 */
export function getUceOperatorDebugSnapshot(now = Date.now()) {
  const holdMs =
    lastStableOutput?.bucket === "known" ? lastCommittedHoldMs : MIN_HOLD_KNOWN_MS;
  const holdRemainingMs =
    lastStableOutput?.bucket === "known"
      ? Math.max(0, holdMs - (now - stableContextEnteredAt))
      : 0;

  const downgradeGraceActive =
    downgradeGraceStartedAt > 0 &&
    now - downgradeGraceStartedAt < DOWNGRADE_GRACE_MS &&
    !!frozenKnownDuringDowngrade;

  const downgradeGraceRemainingMs = downgradeGraceActive
    ? Math.max(0, DOWNGRADE_GRACE_MS - (now - downgradeGraceStartedAt))
    : 0;

  const escalationHeldMs =
    escalationAnchorType && escalationAnchorAt
      ? now - escalationAnchorAt
      : 0;
  const escalationTicks = Math.floor(escalationHeldMs / ESCALATION_INTERVAL_MS);

  return {
    minHoldKnownMs: MIN_HOLD_KNOWN_MS,
    minHoldCccDocGroupMs: MIN_HOLD_CCC_DOC_GROUP_MS,
    lastCommittedHoldMs: lastStableOutput?.bucket === "known" ? holdMs : null,
    escalationIntervalMs: ESCALATION_INTERVAL_MS,
    escalationTicks,
    escalationHeldMs,
    knownConfidenceFloor: KNOWN_CONFIDENCE_FLOOR,
    downgradeGraceMs: DOWNGRADE_GRACE_MS,
    userPauseMs: USER_PAUSE_MS,
    prevRawTypeForEscalation: escalationAnchorType,
    stableContextEnteredAt,
    lastStableKey: stableIdentityKey(lastStableOutput),
    lastStableContext: lastStableOutput
      ? {
          type: lastStableOutput.type,
          bucket: lastStableOutput.bucket,
          confidence: lastStableOutput.confidence,
        }
      : null,
    rawDetectedContext: lastRawDetectedSnapshot
      ? {
          type: lastRawDetectedSnapshot.type,
          bucket: lastRawDetectedSnapshot.bucket,
          confidence: lastRawDetectedSnapshot.confidence,
        }
      : null,
    proposedContext: lastProposedSnapshot
      ? {
          type: lastProposedSnapshot.type,
          bucket: lastProposedSnapshot.bucket,
          confidence: lastProposedSnapshot.confidence,
        }
      : null,
    holdRemainingMs,
    downgradeGraceActive,
    downgradeGraceRemainingMs,
  };
}
