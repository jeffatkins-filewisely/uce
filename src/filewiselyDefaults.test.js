import assert from "node:assert/strict";
import { test } from "node:test";
import {
  FILEWISELY_DEFAULT_ANON_KEY,
  FILEWISELY_DEFAULT_INGEST_URL,
  FILEWISELY_DEFAULT_HANDSHAKE_URL,
  parseUceConnectParams,
  resolveHandshakeClaimUrl,
} from "./filewiselyDefaults.js";

test("parses business_id-only connect link", () => {
  const p = parseUceConnectParams(
    "uce://connect?business_id=550e8400-e29b-41d4-a716-446655440000"
  );
  assert.equal(p.businessId, "550e8400-e29b-41d4-a716-446655440000");
  assert.equal(p.handshakeToken, null);
});

test("treats non-uuid token as handshake", () => {
  const p = parseUceConnectParams("uce://connect?token=shop-pair-abc123");
  assert.equal(p.handshakeToken, "shop-pair-abc123");
  assert.equal(p.businessId, null);
});

test("accepts all three codes", () => {
  const p = parseUceConnectParams(
    "uce://connect?business_id=550e8400-e29b-41d4-a716-446655440000&backend_url=https://example.test/functions/v1/uce-ingest&anon_key=eyJtest"
  );
  assert.equal(p.anonKey, "eyJtest");
  assert.match(p.backendUrl, /uce-ingest/);
});

test("handshake URL falls back to production", () => {
  const url = resolveHandshakeClaimUrl("", "", "");
  assert.equal(url, FILEWISELY_DEFAULT_HANDSHAKE_URL);
  assert.match(FILEWISELY_DEFAULT_INGEST_URL, /uce-ingest/);
});

test("production anon key is the FileWisely publishable JWT", () => {
  assert.match(FILEWISELY_DEFAULT_ANON_KEY, /^eyJ/);
  const payload = JSON.parse(
    Buffer.from(FILEWISELY_DEFAULT_ANON_KEY.split(".")[1], "base64url").toString(
      "utf8"
    )
  );
  assert.equal(payload.ref, "pujwbzqnoevqxrwipnwo");
  assert.equal(payload.role, "anon");
});
