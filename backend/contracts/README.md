# Edge function contracts (Zod)

Copy into the FileWisely / Lovable Supabase functions repo.

## Usage in `ccc-package-claim-batch`

```ts
import { parseClaimBatchRequest } from "../contracts/cccPackageClaimBatch.ts";

const parsed = parseClaimBatchRequest(await req.json());
if (!parsed.ok) {
  return new Response(
    JSON.stringify({
      error: parsed.error,
      missing_fields: parsed.missing_fields,
      hint: "Send { business_id, device_id, limit } in JSON body",
    }),
    { status: 400, headers: { "Content-Type": "application/json" } },
  );
}
const { business_id, device_id, limit } = parsed.data;
```

## Usage in `uce-ingest` (heartbeat branch)

```ts
import { parseHeartbeatRequest } from "../contracts/uceIngestHeartbeat.ts";

if (body?.action === "heartbeat") {
  const parsed = parseHeartbeatRequest(body);
  if (!parsed.ok) {
    return new Response(JSON.stringify(parsed), { status: 400 });
  }
  // ...
}
```

Requires `zod` in the edge bundle (standard in Lovable projects).

## Claim-batch **response** items (mirror_file)

Every claimed job in `items[]` **must** include ack identity fields the desktop echoes on `ccc-package-ack`:

| Field | Required | Notes |
|-------|----------|--------|
| `queue_id` | yes | UUID |
| `source_table` | yes* | e.g. `business_documents`, `business_intake_links` |
| `source_id` | yes* | Source row UUID / id |
| `signed_url` | mirror only | HTTPS download |
| `ro_folder` / `filename_hint` | mirror only | Local path |

\*Sidekick v0.1.63+ infers `source_table=business_documents` when missing on mirror jobs (bridge for backfill/reprocess paths). **Still fix enqueue** — route all paths through one canonical payload builder; do not rely on inference long-term.

Aliases accepted by desktop: `source: { table, id }`, `document_id`, `sourceTable` / `sourceId`.
