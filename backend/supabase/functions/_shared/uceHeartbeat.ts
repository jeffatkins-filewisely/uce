/**
 * UCE ingest heartbeat → upsert uce_devices (service role).
 * Import from uce-ingest/index.ts when body.action === "heartbeat".
 */
import { SupabaseClient } from "https://esm.sh/@supabase/supabase-js@2.49.1";

const DEVICES_TABLE = Deno.env.get("UCE_DEVICES_TABLE") || "uce_devices";

export type DeviceHealthPayload = {
  tray_state?: string;
  tenant_configured?: boolean;
  business_id_short?: string;
  heartbeat_ok?: boolean;
  heartbeat_stale?: boolean;
  last_heartbeat_unix_ms?: number;
  last_heartbeat_category?: string;
  ccc_sync_paused?: boolean;
  ccc_sync_offline?: boolean;
  ccc_syncing_count?: number;
  last_ccc_sync_unix_ms?: number;
  pending_uploads?: number;
  spool_pending?: number;
  last_upload_unix_ms?: number;
  agent_version?: string;
};

export type HeartbeatBody = {
  action: "heartbeat";
  business_id: string;
  device_id: string;
  device_name?: string;
  agent_version?: string;
  os_info?: string;
  user_id?: string;
  ccc_package_capable?: boolean;
  ccc_package_root?: string;
  device_health?: DeviceHealthPayload | null;
};

function msToIso(ms: number | undefined | null): string | null {
  if (ms == null || !Number.isFinite(ms) || ms <= 0) return null;
  return new Date(ms).toISOString();
}

export async function handleUceHeartbeat(
  supabase: SupabaseClient,
  body: HeartbeatBody,
): Promise<{ status: number; body: Record<string, unknown> }> {
  const businessId = String(body.business_id || "").trim();
  const deviceId = String(body.device_id || "").trim();
  if (!businessId || !deviceId) {
    return {
      status: 400,
      body: { error: "business_id and device_id required for heartbeat" },
    };
  }

  const dh = body.device_health ?? null;
  const now = new Date().toISOString();

  // Field-preserving upsert (mirrors the deployed uce-ingest heartbeat handler).
  // A lightweight Rust fallback heartbeat (sleep/wake, frozen WebView) sends no
  // device_health and an empty user_id; those keys are OMITTED here so the
  // upsert does not clobber the richer values written by the JS heartbeat.
  const row: Record<string, unknown> = {
    business_id: businessId,
    device_id: deviceId,
    device_name: body.device_name ?? null,
    agent_version: body.agent_version ?? dh?.agent_version ?? null,
    os_info: body.os_info ?? null,
    last_seen_at: now,
    ccc_package_capable: !!body.ccc_package_capable,
    ccc_package_root: body.ccc_package_root ?? null,
    updated_at: now,
  };

  // Only set user_id when the caller actually knows it. Empty/absent leaves the
  // existing pairing intact (the live handler additionally resolves it from a
  // recent handshake — omitted in this reference copy).
  const trimmedUserId = String(body.user_id ?? "").trim();
  if (trimmedUserId) {
    row.user_id = trimmedUserId;
  }

  // Only overwrite health when this heartbeat carried it.
  if (dh && typeof dh === "object" && !Array.isArray(dh)) {
    row.device_health = dh;
    row.tray_state = dh.tray_state ?? null;
    row.tenant_configured = dh.tenant_configured ?? null;
    row.heartbeat_ok = dh.heartbeat_ok ?? null;
    row.heartbeat_stale = dh.heartbeat_stale ?? null;
    row.last_heartbeat_category = dh.last_heartbeat_category ?? null;
    row.ccc_sync_paused = dh.ccc_sync_paused ?? false;
    row.ccc_sync_offline = dh.ccc_sync_offline ?? false;
    row.ccc_syncing_count = dh.ccc_syncing_count ?? 0;
    row.pending_uploads = dh.pending_uploads ?? 0;
    row.spool_pending = dh.spool_pending ?? 0;
    row.last_ccc_sync_at = msToIso(dh.last_ccc_sync_unix_ms);
    row.last_upload_at = msToIso(dh.last_upload_unix_ms);
  }

  const { error } = await supabase
    .from(DEVICES_TABLE)
    .upsert(row, { onConflict: "business_id,device_id" });

  if (error) {
    console.error("UCE_HEARTBEAT_UPSERT_ERR", error.message);
    return {
      status: 500,
      body: { error: "device upsert failed", detail: error.message },
    };
  }

  // Optional: set update_available when a newer MSI is published (implement in your releases table)
  const updateAvailable = false;

  return {
    status: 200,
    body: {
      ok: true,
      update_available: updateAvailable,
      device_id: deviceId,
    },
  };
}

export function isHeartbeatBody(
  body: Record<string, unknown>,
): body is HeartbeatBody {
  return body?.action === "heartbeat";
}
