-- UCE device registry + device_health persistence (Phase C)
-- Run in Supabase SQL Editor or via supabase db push

-- ---------------------------------------------------------------------------
-- A) New table (use when you do NOT already have a UCE devices table)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS public.uce_devices (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  business_id uuid NOT NULL,
  device_id text NOT NULL,
  device_name text,
  agent_version text,
  os_info text,
  user_id text,
  last_seen_at timestamptz NOT NULL DEFAULT now(),
  ccc_package_capable boolean NOT NULL DEFAULT false,
  ccc_package_root text,
  device_health jsonb,
  -- flattened for filters / Lovable tables (mirrors device_health snapshot)
  tray_state text,
  tenant_configured boolean,
  heartbeat_ok boolean,
  heartbeat_stale boolean,
  last_heartbeat_category text,
  ccc_sync_paused boolean,
  ccc_sync_offline boolean,
  ccc_syncing_count integer NOT NULL DEFAULT 0,
  pending_uploads integer NOT NULL DEFAULT 0,
  spool_pending integer NOT NULL DEFAULT 0,
  last_ccc_sync_at timestamptz,
  last_upload_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT uce_devices_business_device_unique UNIQUE (business_id, device_id)
);

CREATE INDEX IF NOT EXISTS uce_devices_business_id_idx ON public.uce_devices (business_id);
CREATE INDEX IF NOT EXISTS uce_devices_last_seen_idx ON public.uce_devices (last_seen_at DESC);
CREATE INDEX IF NOT EXISTS uce_devices_tray_state_idx ON public.uce_devices (tray_state);

COMMENT ON TABLE public.uce_devices IS 'UCE Sidekick desktops — updated on each ingest heartbeat';

-- RLS: adjust policies to match your auth (business_members, profiles, etc.)
ALTER TABLE public.uce_devices ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS uce_devices_select_member ON public.uce_devices;
CREATE POLICY uce_devices_select_member ON public.uce_devices
  FOR SELECT TO authenticated
  USING (
    business_id IN (
      SELECT bm.business_id FROM public.business_members bm
      WHERE bm.user_id = auth.uid()
    )
  );

DROP POLICY IF EXISTS uce_devices_service_all ON public.uce_devices;
CREATE POLICY uce_devices_service_all ON public.uce_devices
  FOR ALL TO service_role
  USING (true)
  WITH CHECK (true);

-- Anon cannot read devices (edge function uses service role for upsert)
REVOKE ALL ON public.uce_devices FROM anon;

-- Admin view for Lovable / dashboards
CREATE OR REPLACE VIEW public.uce_devices_health_v AS
SELECT
  d.id,
  d.business_id,
  d.device_id,
  d.device_name,
  d.agent_version,
  d.os_info,
  d.last_seen_at,
  d.tray_state,
  d.heartbeat_stale,
  d.heartbeat_ok,
  d.ccc_sync_paused,
  d.ccc_sync_offline,
  d.ccc_syncing_count,
  d.pending_uploads,
  d.spool_pending,
  d.ccc_package_capable,
  d.ccc_package_root,
  d.last_ccc_sync_at,
  d.last_upload_at,
  d.device_health,
  (now() - d.last_seen_at) AS time_since_seen,
  CASE
    WHEN d.last_seen_at < now() - interval '12 minutes' THEN 'stale'
    WHEN d.tray_state = 'red' THEN 'critical'
    WHEN d.tray_state = 'yellow' THEN 'attention'
    ELSE 'healthy'
  END AS support_status
FROM public.uce_devices d;

COMMENT ON VIEW public.uce_devices_health_v IS 'Portal: UCE device health at a glance';

GRANT SELECT ON public.uce_devices_health_v TO authenticated;

-- ---------------------------------------------------------------------------
-- B) ALTER existing table (if your project already uses another name)
-- ---------------------------------------------------------------------------
-- Example: rename uce_devices → your table, or run:
--
-- ALTER TABLE public.devices ADD COLUMN IF NOT EXISTS device_health jsonb;
-- ALTER TABLE public.devices ADD COLUMN IF NOT EXISTS tray_state text;
-- ALTER TABLE public.devices ADD COLUMN IF NOT EXISTS spool_pending integer DEFAULT 0;
-- ... (mirror columns from CREATE TABLE above)
--
-- Then point uce-ingest upsert at your table name in _shared/uceHeartbeat.ts
