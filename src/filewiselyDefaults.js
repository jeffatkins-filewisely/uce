/** Production FileWisely ingest — same for Anaheim and every other shop. */
export const FILEWISELY_DEFAULT_INGEST_URL =
  "https://pujwbzqnoevqxrwipnwo.supabase.co/functions/v1/uce-ingest";

export const FILEWISELY_DEFAULT_HANDSHAKE_URL =
  "https://pujwbzqnoevqxrwipnwo.supabase.co/functions/v1/uce-claim-handshake";

export function isValidUuid(value) {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
    String(value).trim()
  );
}

function decodeIfEncoded(raw) {
  const s = String(raw || "").trim();
  if (!s.toLowerCase().startsWith("uce%3a")) return s;
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

function normalizeUceUrl(urlStr) {
  const s = decodeIfEncoded(urlStr);
  if (/^uce:/i.test(s)) {
    const rest = s.replace(/^uce:/i, "").replace(/^\/+/, "");
    return `http://uce.invalid/${rest}`;
  }
  return s;
}

function firstParam(u, keys) {
  for (const k of keys) {
    const v = (u.searchParams.get(k) || "").trim();
    if (v) return v;
  }
  return "";
}

/**
 * Parse `uce://connect?...` or a FileWisely https landing URL with the same query.
 * `token` / `code` that is not a UUID is treated as a handshake token (portal
 * "Connect to computer" often sends `token=` instead of `handshake_token=`).
 */
export function parseUceConnectParams(urlStr) {
  try {
    const s = String(urlStr || "").trim();
    if (!s) return null;
    const normalized = normalizeUceUrl(s);
    if (!/^https?:/i.test(normalized) && !/^uce:/i.test(s)) return null;
    const u = new URL(normalized);
    const host = (u.hostname || "").toLowerCase();
    const path = (u.pathname || "").replace(/^\/+|\/+$/g, "").toLowerCase();
    const isUce = host === "uce.invalid" || /^uce:/i.test(s);
    const isFilewisely =
      host.includes("filewisely") || host.includes("supabase.co");
    const pathOk =
      !path ||
      path === "connect" ||
      path.includes("uce-connect") ||
      path.includes("connect-uce") ||
      path.endsWith("/connect") ||
      path.includes("uce/connect");
    if (isUce && !pathOk) return null;
    if (!isUce && !isFilewisely && !pathOk) return null;

    let handshakeToken =
      firstParam(u, ["handshake_token", "claim_token"]) || null;
    const tokenOrCode = firstParam(u, ["token", "code"]);
    let id =
      firstParam(u, ["business_id", "businessId", "shop_id"]) || null;
    const backendUrl = firstParam(u, [
      "backend_url",
      "ingest_url",
      "upload_url",
      "backendUrl",
    ]);
    const anonKey = firstParam(u, [
      "anon_key",
      "api_key",
      "apikey",
      "supabase_anon_key",
      "anonKey",
    ]);
    if (tokenOrCode) {
      if (isValidUuid(tokenOrCode)) {
        if (!id) id = tokenOrCode;
      } else if (!handshakeToken) {
        handshakeToken = tokenOrCode;
      }
    }
    if (!handshakeToken && !id) return null;
    return {
      businessId: id,
      backendUrl,
      anonKey,
      handshakeToken,
    };
  } catch {
    return null;
  }
}

export function resolveHandshakeClaimUrl(backendUrlHint, viteUploadUrl, viteHandshakeUrl) {
  const explicit = String(viteHandshakeUrl || "").trim();
  if (explicit) return explicit;
  const base = (
    (backendUrlHint && String(backendUrlHint).trim()) ||
    String(viteUploadUrl || "").trim() ||
    FILEWISELY_DEFAULT_INGEST_URL
  ).trim();
  try {
    const u = new URL(base);
    u.pathname = u.pathname.replace(/uce-ingest/i, "uce-claim-handshake");
    return u.toString();
  } catch {
    return FILEWISELY_DEFAULT_HANDSHAKE_URL;
  }
}
