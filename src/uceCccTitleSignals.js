/**
 * Shared CCC-oriented title heuristics (used by RO “seen” tracking and context recognition).
 * @param {string} title
 * @returns {{ key: string, label: string }[]}
 */
export function inferCccDocSignalsFromTitle(title) {
  if (!title || typeof title !== "string") return [];
  const t = title.toLowerCase();
  const out = [];
  const seen = new Set();

  if (/\bteardown\b/.test(t)) {
    seen.add("teardown");
    out.push({ key: "teardown", label: "Teardown photos" });
  }
  if (/\bfinal\s*bill\b/.test(t)) {
    seen.add("final_bill");
    out.push({ key: "final_bill", label: "Final Bill" });
  }
  const suppM =
    t.match(/\bsupplement\s*#?\s*(\d{1,2})\b/) ||
    t.match(/\bsupp\.?\s*#?\s*(\d{1,2})\b/) ||
    t.match(/\bsuppl\.?\s*#?\s*(\d{1,2})\b/);
  if (suppM) {
    const n = Math.min(9, Math.max(1, parseInt(suppM[1], 10)));
    const k = `supplement_${n}`;
    if (!seen.has(k)) {
      seen.add(k);
      out.push({ key: k, label: `Supplement #${n}` });
    }
  }
  if (
    /\bprint\b/.test(t) ||
    /\bprinting\b/.test(t) ||
    /\bprint\s*preview\b/.test(t) ||
    /\bforms\b/.test(t)
  ) {
    if (!seen.has("print_panel")) {
      seen.add("print_panel");
      out.push({ key: "print_panel", label: "Print / documents panel" });
    }
  }
  if (
    /\bpreview\b/.test(t) ||
    /\bthumbnail\b/.test(t) ||
    /\bdocument\s*preview\b/.test(t)
  ) {
    if (!seen.has("document_preview")) {
      seen.add("document_preview");
      out.push({ key: "document_preview", label: "Document preview" });
    }
  }
  if (
    /\bestimate\b/.test(t) &&
    !/\bsupplement\b/.test(t) &&
    !/\bsupp\b/.test(t) &&
    !suppM
  ) {
    if (!seen.has("estimate")) {
      seen.add("estimate");
      out.push({ key: "estimate", label: "Initial estimate" });
    }
  }
  return out;
}
