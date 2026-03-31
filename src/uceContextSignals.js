/**
 * Lightweight local signals for context recognition (v1).
 * Extend here when Rust exposes folder telemetry or print state on the context object.
 */

let lastFwPdfIncomingAt = 0;
let lastOfficePrintPromptAt = 0;
let lastUserActivityAt = Date.now();

export function recordUserActivity() {
  lastUserActivityAt = Date.now();
}

export function recordFwPdfIncomingForContext() {
  lastFwPdfIncomingAt = Date.now();
}

export function recordOfficePrintPromptForContext() {
  lastOfficePrintPromptAt = Date.now();
}

/**
 * @returns {{
 *   msSinceFwPdfIncoming: number,
 *   msSinceOfficePrintPrompt: number,
 *   msSinceUserActivity: number,
 * }}
 */
export function getUceRecognitionSignals() {
  const now = Date.now();
  return {
    msSinceFwPdfIncoming: now - lastFwPdfIncomingAt,
    msSinceOfficePrintPrompt: now - lastOfficePrintPromptAt,
    msSinceUserActivity: now - lastUserActivityAt,
  };
}
