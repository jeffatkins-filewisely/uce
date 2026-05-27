# Lovable prompt — UCE Devices health page

Paste this into Lovable for the FileWisely portal:

---

Add an admin page **Settings → UCE Devices** that lists shop desktops running UCE Sidekick.

**Data:** Supabase view `uce_devices_health_v`, filtered by the current user's `business_id` from auth context (same pattern as other shop-scoped tables).

**Columns:**

| Column | Source |
|--------|--------|
| Device | `device_name` or `device_id` |
| Version | `agent_version` |
| Status | `support_status` badge (healthy / attention / critical / stale) |
| Tray | `tray_state` colored dot (green / yellow / red) |
| Last seen | `last_seen_at` relative time |
| CCC sync | `ccc_sync_offline` → Offline; `ccc_sync_paused` → Paused; else OK |
| Upload queue | `pending_uploads` + `spool_pending` (show spool if > 0) |
| CCC import path | `ccc_package_root` (truncate with tooltip) |

**Refresh:** auto-refresh every 60s on the page.

**Empty state:** "No UCE devices yet — install Sidekick and Connect from the tray."

**Copy component:** use `lovable/src/components/UceDevicesHealthTable.tsx` from the Sidekick repo `backend/lovable/` folder.

**RLS:** view is granted to `authenticated`; users only see their business via existing `business_members` policy on `uce_devices`.

---

## Prerequisites (Supabase)

1. Run migration `backend/supabase/migrations/20260526120000_uce_device_health.sql`
2. Merge heartbeat handler into `uce-ingest` (see `backend/supabase/functions/uce-ingest/MERGE_HEARTBEAT.md`)
3. Deploy edge function
