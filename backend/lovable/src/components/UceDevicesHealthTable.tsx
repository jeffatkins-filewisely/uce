/**
 * FileWisely portal — UCE device health table.
 * Requires: supabase client, businessId from your auth context.
 */
import { useCallback, useEffect, useState } from "react";
import type { SupabaseClient } from "@supabase/supabase-js";

export type UceDeviceHealthRow = {
  id: string;
  business_id: string;
  device_id: string;
  device_name: string | null;
  agent_version: string | null;
  os_info: string | null;
  last_seen_at: string;
  tray_state: string | null;
  heartbeat_stale: boolean | null;
  heartbeat_ok: boolean | null;
  ccc_sync_paused: boolean | null;
  ccc_sync_offline: boolean | null;
  ccc_syncing_count: number | null;
  pending_uploads: number | null;
  spool_pending: number | null;
  ccc_package_capable: boolean | null;
  ccc_package_root: string | null;
  last_ccc_sync_at: string | null;
  last_upload_at: string | null;
  support_status: string | null;
  time_since_seen: string | null;
};

function trayDot(state: string | null) {
  const c =
    state === "green"
      ? "#22c55e"
      : state === "yellow"
        ? "#eab308"
        : state === "red"
          ? "#ef4444"
          : "#94a3b8";
  return (
    <span
      title={state ?? "unknown"}
      style={{
        display: "inline-block",
        width: 10,
        height: 10,
        borderRadius: "50%",
        backgroundColor: c,
        marginRight: 6,
      }}
    />
  );
}

function statusBadge(status: string | null) {
  const label = status ?? "unknown";
  const bg =
    label === "healthy"
      ? "#dcfce7"
      : label === "attention"
        ? "#fef9c3"
        : label === "critical" || label === "stale"
          ? "#fee2e2"
          : "#f1f5f9";
  return (
    <span
      style={{
        padding: "2px 8px",
        borderRadius: 6,
        fontSize: 12,
        backgroundColor: bg,
        textTransform: "capitalize",
      }}
    >
      {label}
    </span>
  );
}

function formatRelative(iso: string | null) {
  if (!iso) return "—";
  const ms = Date.now() - new Date(iso).getTime();
  if (ms < 60_000) return "just now";
  if (ms < 3_600_000) return `${Math.floor(ms / 60_000)}m ago`;
  if (ms < 86_400_000) return `${Math.floor(ms / 3_600_000)}h ago`;
  return `${Math.floor(ms / 86_400_000)}d ago`;
}

function cccSyncLabel(row: UceDeviceHealthRow) {
  if (row.ccc_sync_paused) return "Paused";
  if (row.ccc_sync_offline) return "Offline";
  if ((row.ccc_syncing_count ?? 0) > 0) return `${row.ccc_syncing_count} syncing`;
  return "OK";
}

type Props = {
  supabase: SupabaseClient;
  businessId: string;
  pollMs?: number;
};

export function UceDevicesHealthTable({
  supabase,
  businessId,
  pollMs = 60_000,
}: Props) {
  const [rows, setRows] = useState<UceDeviceHealthRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!businessId) {
      setRows([]);
      setLoading(false);
      return;
    }
    setError(null);
    const { data, error: err } = await supabase
      .from("uce_devices_health_v")
      .select("*")
      .eq("business_id", businessId)
      .order("last_seen_at", { ascending: false });

    if (err) {
      setError(err.message);
      setRows([]);
    } else {
      setRows((data as UceDeviceHealthRow[]) ?? []);
    }
    setLoading(false);
  }, [supabase, businessId]);

  useEffect(() => {
    void load();
    const id = setInterval(() => void load(), pollMs);
    return () => clearInterval(id);
  }, [load, pollMs]);

  if (loading) {
    return <p style={{ color: "#64748b" }}>Loading UCE devices…</p>;
  }

  if (error) {
    return (
      <p style={{ color: "#b91c1c" }}>
        Could not load devices: {error}. Run the Supabase migration and deploy
        uce-ingest heartbeat handler.
      </p>
    );
  }

  if (rows.length === 0) {
    return (
      <p style={{ color: "#64748b" }}>
        No UCE devices yet — install Sidekick at the shop and use tray → Connect.
      </p>
    );
  }

  return (
    <div style={{ overflowX: "auto" }}>
      <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 14 }}>
        <thead>
          <tr style={{ textAlign: "left", borderBottom: "1px solid #e2e8f0" }}>
            <th style={{ padding: 8 }}>Device</th>
            <th style={{ padding: 8 }}>Version</th>
            <th style={{ padding: 8 }}>Status</th>
            <th style={{ padding: 8 }}>Tray</th>
            <th style={{ padding: 8 }}>Last seen</th>
            <th style={{ padding: 8 }}>CCC sync</th>
            <th style={{ padding: 8 }}>Queue</th>
            <th style={{ padding: 8 }}>CCC import</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.id} style={{ borderBottom: "1px solid #f1f5f9" }}>
              <td style={{ padding: 8 }}>
                {row.device_name?.trim() || row.device_id}
              </td>
              <td style={{ padding: 8 }}>{row.agent_version ?? "—"}</td>
              <td style={{ padding: 8 }}>
                {statusBadge(row.support_status)}
              </td>
              <td style={{ padding: 8 }}>
                {trayDot(row.tray_state)}
                {row.tray_state ?? "—"}
              </td>
              <td style={{ padding: 8 }}>
                {formatRelative(row.last_seen_at)}
              </td>
              <td style={{ padding: 8 }}>{cccSyncLabel(row)}</td>
              <td style={{ padding: 8 }}>
                {row.pending_uploads ?? 0}
                {(row.spool_pending ?? 0) > 0
                  ? ` (+${row.spool_pending} spool)`
                  : ""}
              </td>
              <td
                style={{
                  padding: 8,
                  maxWidth: 200,
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
                title={row.ccc_package_root ?? ""}
              >
                {row.ccc_package_capable
                  ? row.ccc_package_root ?? "—"
                  : "—"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default UceDevicesHealthTable;
