/**
 * Flag-pay / work-order helpers.
 * Reads CCC ONE and Mitchell work-order signals (window title, filename, PDF text)
 * and builds the payload FileWisely uses to set RO repair flags to the correct hours.
 */

const HOUR_MAX = 40;
const HOUR_MIN = 0.1;

const WORK_ORDER_RE =
  /\b(work\s*order|workorder|w\.?\s*o\.?\b|wo[#:\s-]|labor\s+assign|labor\s+op|flag\s*hours?|tech(?:nician)?\s+hours?|labor\s+hours?)\b/i;

const MITCHELL_RE = /\b(mitchell|ultramate|cloud\s*estimating|workcenter)\b/i;
const CCC_RE = /\b(ccc(?:\s*one)?|cccone|workfile)\b/i;

const OP_ALIASES = [
  ["refinish", /\b(refinish|paint|ref)\b/i],
  ["body", /\b(body|sheet\s*metal|sm)\b/i],
  ["mechanical", /\b(mech(?:anical)?|mechanical)\b/i],
  ["frame", /\b(frame|struct(?:ural)?)\b/i],
  ["detail", /\b(detail|cleanup)\b/i],
  ["aluminum", /\baluminum\b/i],
  ["labor", /\blabor\b/i],
];

export function inferSourceSystem(sourceApp = "", windowTitle = "", filePath = "") {
  const blob = `${sourceApp} ${windowTitle} ${filePath}`.toLowerCase();
  if (MITCHELL_RE.test(blob)) return "mitchell";
  if (CCC_RE.test(blob) || /\btemp\\ccc\b/i.test(blob) || /\\ccc\\/i.test(blob)) {
    return "ccc";
  }
  return "unknown";
}

export function looksLikeWorkOrder({
  windowTitle = "",
  filePath = "",
  matchedRule = "",
  extractedText = "",
} = {}) {
  const rule = String(matchedRule || "").toLowerCase();
  if (rule.includes("work_order") || rule.includes("mitchell")) return true;
  const blob = `${windowTitle} ${filePath} ${extractedText}`;
  return WORK_ORDER_RE.test(blob);
}

export function extractRoFromWorkOrderText(text) {
  const t = String(text || "").replace(/\u00a0/g, " ");
  const patterns = [
    /\bWO[#:\s-]*(\d{4,7})\b/i,
    /\bW\.?O\.?[#:\s-]*(\d{4,7})\b/i,
    /\bRO[#:\s-]*(\d{4,7})\b/i,
    /\bRepair\s*Order[#:\s]*(\d{4,7})\b/i,
    /\bWork\s*Order[#:\s-]*(\d{4,7})\b/i,
    /^(\d{4,6})\b/,
  ];
  for (const re of patterns) {
    const m = t.match(re);
    if (m && m[1]) return m[1];
  }
  return "";
}

function parseHourToken(raw) {
  const n = Number.parseFloat(String(raw));
  if (!Number.isFinite(n) || n < HOUR_MIN || n > HOUR_MAX) return null;
  return Math.round(n * 100) / 100;
}

function inferOpNear(text, index) {
  const start = Math.max(0, index - 18);
  const window = text.slice(start, Math.min(text.length, index + 8));
  for (const [op, re] of OP_ALIASES) {
    if (re.test(window)) return op;
  }
  return "labor";
}

/**
 * @param {string} text
 * @returns {{ totalHours: number|null, laborLines: { op: string, hours: number, tech: string }[] }}
 */
export function extractFlagHoursFromText(text) {
  const t = String(text || "").replace(/\u00a0/g, " ");
  const lines = [];
  const seen = new Set();
  const explicitTotals = [];

  const totalRes = [
    /\bflag(?:ged)?(?:\s*hours?)?[:\s]+(\d{1,2}(?:\.\d{1,2})?)\b/gi,
    /\b(\d{1,2}(?:\.\d{1,2})?)\s*(?:flag(?:ged)?\s*hours?|flagged)\b/gi,
    /\b(?:labor|tech(?:nician)?)\s*hours?[:\s]+(\d{1,2}(?:\.\d{1,2})?)\b/gi,
  ];
  for (const re of totalRes) {
    re.lastIndex = 0;
    let m;
    while ((m = re.exec(t)) !== null) {
      const hours = parseHourToken(m[1]);
      if (hours != null) explicitTotals.push(hours);
    }
  }

  const lineRes = [
    /\b(\d{1,2}(?:\.\d{1,2})?)\s*(?:hr|hrs|hours?)\b/gi,
    /\b(?:body|refinish|paint|mech(?:anical)?|frame|struct(?:ural)?|detail|aluminum|labor)\s*[:\-]?\s*(\d{1,2}(?:\.\d{1,2})?)\b/gi,
  ];
  for (const re of lineRes) {
    re.lastIndex = 0;
    let m;
    while ((m = re.exec(t)) !== null) {
      const hours = parseHourToken(m[1]);
      if (hours == null) continue;
      const near = t.slice(Math.max(0, m.index - 16), m.index + 12);
      if (/\bflag/i.test(near)) continue;
      const op = inferOpNear(t, m.index);
      const key = `${op}:${hours}`;
      if (seen.has(key)) continue;
      seen.add(key);
      lines.push({ op, hours, tech: "" });
    }
  }

  let totalHours = null;
  if (explicitTotals.length) {
    totalHours = Math.max(...explicitTotals);
  } else if (lines.length) {
    const ops = lines.filter((l) => l.op !== "labor");
    totalHours = ops.length
      ? ops.reduce((sum, l) => sum + l.hours, 0)
      : Math.max(...lines.map((l) => l.hours));
  }
  if (totalHours != null) {
    totalHours = Math.round(totalHours * 100) / 100;
    if (totalHours > 80) {
      totalHours = Math.max(...lines.map((l) => l.hours), ...explicitTotals);
    }
  }

  return { totalHours, laborLines: lines };
}

/**
 * @returns {null | {
 *   source_system: string,
 *   is_work_order: boolean,
 *   repair_order_number: string,
 *   flag_hours_total: number|null,
 *   labor_lines: object[],
 *   evidence: string[],
 *   update_ro_repair_flags: boolean,
 *   window_title: string,
 *   file_path: string
 * }}
 */
export function buildFlagPaySnapshot({
  sourceApp = "",
  windowTitle = "",
  filePath = "",
  matchedRule = "",
  extractedText = "",
  knownRo = "",
} = {}) {
  const title = String(windowTitle || "");
  const path = String(filePath || "");
  const text = String(extractedText || "");
  const isWo = looksLikeWorkOrder({
    windowTitle: title,
    filePath: path,
    matchedRule,
    extractedText: text,
  });
  const fromTitle = extractFlagHoursFromText(`${title} ${path}`);
  const fromPdf = extractFlagHoursFromText(text);
  const laborLines = fromPdf.laborLines.length
    ? fromPdf.laborLines
    : fromTitle.laborLines;
  const totalHours =
    fromPdf.totalHours != null ? fromPdf.totalHours : fromTitle.totalHours;
  const ro =
    String(knownRo || "").trim() ||
    extractRoFromWorkOrderText(title) ||
    extractRoFromWorkOrderText(path) ||
    extractRoFromWorkOrderText(text);

  if (!isWo && totalHours == null && !ro) return null;

  const evidence = [];
  if (fromTitle.totalHours != null) evidence.push("window_or_filename");
  if (fromPdf.totalHours != null) evidence.push("pdf_text");
  if (isWo) evidence.push("work_order_surface");

  return {
    source_system: inferSourceSystem(sourceApp, title, path),
    is_work_order: isWo,
    repair_order_number: ro,
    flag_hours_total: totalHours,
    labor_lines: laborLines,
    evidence,
    update_ro_repair_flags: totalHours != null && !!ro,
    window_title: title,
    file_path: path,
  };
}
