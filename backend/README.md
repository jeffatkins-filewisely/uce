# FileWisely backend — UCE device health (Phase C)

**Production backend** lives in **https://github.com/jeffatkins-filewisely/filewisely** (Lovable + Supabase). This `backend/` folder in **uce** is a mirror/deploy reference — edit edge functions in **filewisely**, then sync contracts here.

UCE desktop **already sends** `device_health` on every heartbeat. This package:

1. Adds DB columns / `uce_devices` table
2. Patches **`uce-ingest`** to persist heartbeat + health
3. Provides a **Lovable-ready** devices table UI

## API contracts

Desktop + edge must stay aligned. See **`docs/CONTRACTS.md`** and copy **`backend/contracts/*.ts`** into your Supabase functions repo (Zod validation, structured 400s).

## Deploy order

### 1. SQL migration

In Supabase → **SQL Editor**, run:

`supabase/migrations/20260526120000_uce_device_health.sql`

If you already have a devices table under another name, use **section B** at the bottom of that file instead of creating `uce_devices`.

### 2. Edge function

Copy into your repo:

- `supabase/functions/_shared/uceHeartbeat.ts`
- Merge `supabase/functions/uce-ingest/MERGE_HEARTBEAT.md` into your existing **`uce-ingest/index.ts`**

Or replace only the heartbeat branch — do **not** remove capture/upload handling.

Deploy:

```bash
supabase functions deploy uce-ingest
```

Secrets (already typical): `SUPABASE_URL`, `SUPABASE_SERVICE_ROLE_KEY`.

### 3. Lovable UI

Open `lovable/LOVABLE_PROMPT.md` — paste into Lovable chat, or copy `lovable/src/components/UceDevicesHealthTable.tsx` into your portal.

Route suggestion: **Settings → UCE Devices** (shop admin only).

### 4. Verify

1. Install UCE **v0.1.55+** at a shop PC and Connect tenant.
2. Wait for heartbeat (~5 min) or restart UCE.
3. Supabase → `uce_devices` → row for `device_id` with `device_health` JSON populated.
4. Portal shows tray color, version, spool count.

## Desktop contract

See `docs/DEVICE_HEALTH.md` in the Sidekick repo for the heartbeat JSON shape.
