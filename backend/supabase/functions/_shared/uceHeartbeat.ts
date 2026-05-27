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

  const row: Record<string, unknown> = {
    business_id: businessId,
    device_id: deviceId,
    device_name: body.device_name ?? null,
    agent_version: body.agent_version ?? dh?.agent_version ?? null,
    os_info: body.os_info ?? null,
    user_id: body.user_id ?? null,
    last_seen_at: now,
    ccc_package_capable: !!body.ccc_package_capable,
    ccc_package_root: body.ccc_package_root ?? null,
    device_health: dh,
    tray_state: dh?.tray_state ?? null,
    tenant_configured: dh?.tenant_configured ?? null,
    heartbeat_ok: dh?.heartbeat_ok ?? null,
    heartbeat_stale: dh?.heartbeat_stale ?? null,
    last_heartbeat_category: dh?.last_heartbeat_category ?? null,
    ccc_sync_paused: dh?.ccc_sync_paused ?? false,
    ccc_sync_offline: dh?.ccc_sync_offline ?? false,
    ccc_syncing_count: dh?.ccc_syncing_count ?? 0,
    pending_uploads: dh?.pending_uploads ?? 0,
    spool_pending: dh?.spool_pending ?? 0,
    last_ccc_sync_at: msToIso(dh?.last_ccc_sync_unix_ms),
    last_upload_at: msToIso(dh?.last_upload_unix_ms),
    updated_at: now,
  };

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
