# Phase C — Portal device health (FileWisely / Lovable)

UCE desktop sends `device_health` on every heartbeat. The portal needs to **store** and **display** it.

All deploy artifacts live in **`backend/`** at the repo root.

## Quick deploy

1. **Supabase SQL** — run `backend/supabase/migrations/20260526120000_uce_device_health.sql`
2. **uce-ingest** — merge `backend/supabase/functions/_shared/uceHeartbeat.ts` per `backend/supabase/functions/uce-ingest/MERGE_HEARTBEAT.md`
3. **Lovable** — paste `backend/lovable/LOVABLE_PROMPT.md` or add `UceDevicesHealthTable.tsx` under Settings

## What shops see

After UCE v0.1.55+ is connected, admins see each PC: tray color, agent version, last seen, CCC sync state, upload/spool queue counts, CCC Import path.

## Adjust for your schema

- Table name not `uce_devices`? Set env `UCE_DEVICES_TABLE` on the edge function, or edit `uceHeartbeat.ts`.
- RLS uses different membership table? Edit policies in the migration file section A.
