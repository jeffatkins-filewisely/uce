/**
 * Deterministic supplement sequencing + CCC signal mapping (data only; no UI).
 * @module uceSupplementRoModel
 */

/** @typedef {{ supplement_number: number }} InSystemSupplementEntry */
/** @typedef {{ type: 'sequence_gap', supplement_number: number, confirmed: boolean, possible_source?: string, confidence?: number }} SequenceGapEntry */
/** @typedef {{ seen_in_ccc: true, related_type: string, timestamp: Date, supplement_number?: number }} SeenInCccEntry */
/** @typedef {{ confirmed_missing: true, supplement_number: number|null, source: 'missing_critical' | 'checklist_explicit' }} ConfirmedMissingEntry */
/**
 * @typedef {{
 *   in_system: InSystemSupplementEntry[],
 *   sequence_gaps: SequenceGapEntry[],
 *   seen_in_ccc: SeenInCccEntry[],
 *   confirmed_missing: ConfirmedMissingEntry[],
 * }} RoSupplementTruthModel
 */

const SUPP_LABEL_RE =
  /\bsupplement\s*#?\s*(\d{1,2})\b|\bsupp\.?\s*#?\s*(\d{1,2})\b|\bsuppl\.?\s*#?\s*(\d{1,2})\b/i;

/**
 * @param {string} label
 * @returns {number|null}
 */
export function parseSupplementNumberFromLabel(label) {
  if (!label || typeof label !== "string") return null;
  const m = label.match(SUPP_LABEL_RE);
  if (!m) return null;
  const raw = m[1] ?? m[2] ?? m[3];
  const n = parseInt(raw, 10);
  if (!Number.isFinite(n) || n < 1 || n > 99) return null;
  return n;
}

/**
 * @param {Iterable<string>} labels
 * @returns {number[]}
 */
export function extractSupplementNumbersFromLabels(labels) {
  const out = [];
  for (const lab of labels) {
    const n = parseSupplementNumberFromLabel(String(lab ?? ""));
    if (n != null) out.push(n);
  }
  return out;
}

/**
 * All missing supplement indices between each consecutive pair in the sorted unique list.
 * Example: [1,3,6] → [2,4,5]; [1,2,5] → [3,4].
 * @param {number[]} supplements
 * @returns {number[]}
 */
export function findSequenceGaps(supplements) {
  const sorted = [
    ...new Set(
      [...supplements]
        .filter((n) => Number.isFinite(Number(n)))
        .map((n) => Math.trunc(Number(n)))
    ),
  ].sort((a, b) => a - b);
  const gaps = [];
  for (let i = 1; i < sorted.length; i++) {
    const prev = sorted[i - 1];
    const curr = sorted[i];
    for (let n = prev + 1; n < curr; n++) {
      gaps.push(n);
    }
  }
  return gaps;
}

/**
 * @param {string} key
 * @param {string} label
 * @param {number} firstSeenAtMs
 * @returns {SeenInCccEntry | null}
 */
export function seenInCccPayloadFromSeenKey(key, label, firstSeenAtMs) {
  const ts = new Date(firstSeenAtMs);
  if (key === "print_panel") {
    return { seen_in_ccc: true, related_type: "print_panel", timestamp: ts };
  }
  if (key === "document_preview") {
    return { seen_in_ccc: true, related_type: "document_preview", timestamp: ts };
  }
  const m = /^supplement_(\d+)$/.exec(key);
  if (m) {
    return {
      seen_in_ccc: true,
      related_type: "supplement",
      supplement_number: Number(m[1]),
      timestamp: ts,
    };
  }
  return null;
}

/**
 * @param {Record<string, { label?: string, firstSeenAt?: number }>} localSeenByKey
 * @returns {SeenInCccEntry[]}
 */
export function seenInCccEntriesFromLocalSeen(localSeenByKey) {
  if (!localSeenByKey || typeof localSeenByKey !== "object") return [];
  const out = [];
  for (const key of Object.keys(localSeenByKey)) {
    const rec = localSeenByKey[key];
    if (!rec || typeof rec.firstSeenAt !== "number") continue;
    const p = seenInCccPayloadFromSeenKey(
      key,
      String(rec.label ?? ""),
      rec.firstSeenAt
    );
    if (p) out.push(p);
  }
  return out;
}

/**
 * @param {SeenInCccEntry[]} seen
 * @returns {boolean}
 */
function hasSupplementCorrelationActivity(seen) {
  return seen.some((s) =>
    ["supplement", "print_panel", "document_preview"].includes(s.related_type)
  );
}

/**
 * Applies at most one CCC correlation boost per gap (idempotent if `possible_source` already `ccc_seen`).
 * Confidence: `Math.min(0.7, (gap.confidence ?? 0.5) + 0.2)` — no stacking across repeated calls.
 *
 * @param {SequenceGapEntry[]} gaps
 * @param {SeenInCccEntry[]} seen
 * @returns {SequenceGapEntry[]}
 */
export function correlateGapsWithCccSignals(gaps, seen) {
  if (!gaps.length || !hasSupplementCorrelationActivity(seen)) {
    return gaps.map((g) => ({ ...g }));
  }
  return gaps.map((g) => {
    if (g.possible_source === "ccc_seen") {
      return { ...g };
    }
    const base = g.confidence ?? 0.5;
    return {
      ...g,
      possible_source: "ccc_seen",
      confidence: Math.min(0.7, base + 0.2),
    };
  });
}

/**
 * Strict confirmed missing: explicit server list and/or checklist explicit-missing rows only.
 * @param {string[]} missingCriticalLabels
 * @param {string[]} explicitChecklistMissingLabels
 * @returns {ConfirmedMissingEntry[]}
 */
export function buildConfirmedMissingOnlyExplicit(
  missingCriticalLabels,
  explicitChecklistMissingLabels
) {
  const out = [];
  const seen = new Set();
  const push = (label, source) => {
    const s = String(label ?? "").trim();
    if (!s) return;
    const k = `${source}\0${normalizeLabelKey(s)}`;
    if (seen.has(k)) return;
    seen.add(k);
    const supplement_number = parseSupplementNumberFromLabel(s);
    out.push({
      confirmed_missing: true,
      supplement_number,
      source,
    });
  };
  for (const lab of missingCriticalLabels || []) {
    push(lab, "missing_critical");
  }
  for (const lab of explicitChecklistMissingLabels || []) {
    push(lab, "checklist_explicit");
  }
  return out;
}

/** @param {string} s */
function normalizeLabelKey(s) {
  return s.toLowerCase().replace(/\s+/g, " ").trim();
}

/**
 * Single source of truth for supplement sequencing + CCC mapping + strict missing.
 *
 * @param {{
 *   inSystemLabels: string[],
 *   missingCriticalLabels?: string[],
 *   explicitChecklistMissingLabels?: string[],
 *   localSeenByKey?: Record<string, { label?: string, firstSeenAt?: number }>,
 * }} p
 * @returns {RoSupplementTruthModel}
 */
export function buildRoSupplementTruthModel(p) {
  const inSystemLabels = Array.isArray(p.inSystemLabels) ? p.inSystemLabels : [];
  const nums = extractSupplementNumbersFromLabels(inSystemLabels);
  const uniqueSorted = [...new Set(nums)].sort((a, b) => a - b);
  const in_system = uniqueSorted.map((supplement_number) => ({ supplement_number }));

  const gapNumbers = findSequenceGaps(uniqueSorted);
  const sequence_gaps = gapNumbers.map((supplement_number) => ({
    type: "sequence_gap",
    supplement_number,
    confirmed: false,
  }));

  const seen_in_ccc = seenInCccEntriesFromLocalSeen(p.localSeenByKey || {});

  const correlated = correlateGapsWithCccSignals(sequence_gaps, seen_in_ccc);

  const confirmed_missing = buildConfirmedMissingOnlyExplicit(
    p.missingCriticalLabels,
    p.explicitChecklistMissingLabels
  );

  return {
    in_system,
    sequence_gaps: correlated,
    seen_in_ccc,
    confirmed_missing,
  };
}
