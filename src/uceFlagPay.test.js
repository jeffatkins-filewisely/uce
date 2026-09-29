import assert from "node:assert/strict";
import { test } from "node:test";
import {
  buildFlagPaySnapshot,
  extractFlagHoursFromText,
  extractRoFromWorkOrderText,
  inferSourceSystem,
  looksLikeWorkOrder,
} from "./uceFlagPay.js";

test("detects CCC and Mitchell work-order surfaces", () => {
  assert.equal(
    looksLikeWorkOrder({ windowTitle: "90066 Work Order — Labor" }),
    true
  );
  assert.equal(
    looksLikeWorkOrder({
      windowTitle: "Mitchell Cloud Estimating - Work Order 44112",
    }),
    true
  );
  assert.equal(looksLikeWorkOrder({ matchedRule: "mitchell_work_order" }), true);
  assert.equal(looksLikeWorkOrder({ windowTitle: "Inbox - Outlook" }), false);
});

test("infers source system", () => {
  assert.equal(
    inferSourceSystem("chrome", "Mitchell UltraMate - RO 90066", ""),
    "mitchell"
  );
  assert.equal(inferSourceSystem("ccc.exe", "90066 Estimate", ""), "ccc");
});

test("extracts RO from work-order titles", () => {
  assert.equal(extractRoFromWorkOrderText("Work Order 90066 — Body"), "90066");
  assert.equal(extractRoFromWorkOrderText("WO#44112 Labor Hours"), "44112");
  assert.equal(extractRoFromWorkOrderText("Repair Order 77881"), "77881");
});

test("reads flag hours and labor ops", () => {
  const parsed = extractFlagHoursFromText(
    "Work Order 90066  Body 4.0  Refinish 2.5  Flag hours 6.5"
  );
  assert.equal(parsed.totalHours, 6.5);
  assert.ok(parsed.laborLines.some((l) => l.op === "body" && l.hours === 4));
  assert.ok(parsed.laborLines.some((l) => l.op === "refinish" && l.hours === 2.5));
});

test("builds a FileWisely flag-pay snapshot that can update RO flags", () => {
  const snap = buildFlagPaySnapshot({
    sourceApp: "ccc.exe",
    windowTitle: "90066 Work Order Flag hours 6.5",
    filePath: "C:\\\\FileWisely\\\\Incoming\\\\WO_90066.pdf",
    matchedRule: "ccc_work_order",
  });
  assert.equal(snap.source_system, "ccc");
  assert.equal(snap.is_work_order, true);
  assert.equal(snap.repair_order_number, "90066");
  assert.equal(snap.flag_hours_total, 6.5);
  assert.equal(snap.update_ro_repair_flags, true);
});
