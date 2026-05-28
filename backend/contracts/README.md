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
