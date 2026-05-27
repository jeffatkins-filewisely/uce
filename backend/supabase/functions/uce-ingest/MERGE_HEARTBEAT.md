# Merge into existing `uce-ingest/index.ts`

Add at the top (adjust import path to your `_shared` layout):

```ts
import { createClient } from "https://esm.sh/@supabase/supabase-js@2.49.1";
import { handleUceHeartbeat, isHeartbeatBody } from "../_shared/uceHeartbeat.ts";
```

Inside your `Deno.serve` handler, **before** capture/upload logic:

```ts
const supabase = createClient(
  Deno.env.get("SUPABASE_URL")!,
  Deno.env.get("SUPABASE_SERVICE_ROLE_KEY")!,
);

let body: Record<string, unknown>;
try {
  body = await req.json();
} catch {
  return new Response(JSON.stringify({ error: "invalid json" }), { status: 400 });
}

if (isHeartbeatBody(body)) {
  const { status, body: respBody } = await handleUceHeartbeat(supabase, body);
  return new Response(JSON.stringify(respBody), {
    status,
    headers: { "Content-Type": "application/json", ...corsHeaders },
  });
}

// ... existing capture / upload handling continues ...
```

## Env

| Variable | Default |
|----------|---------|
| `SUPABASE_URL` | required |
| `SUPABASE_SERVICE_ROLE_KEY` | required |
| `UCE_DEVICES_TABLE` | `uce_devices` — set if your table has another name |

## CORS

Reuse your existing `corsHeaders` on the heartbeat response.

## Do not break

- PDF / image capture ingest
- Existing heartbeat fields (`agent_version`, `device_name`) your portal already uses
- `update_available` if you already return it from a releases check — replace `updateAvailable = false` in `uceHeartbeat.ts`
