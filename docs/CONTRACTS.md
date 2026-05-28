# UCE ↔ FileWisely edge API contracts

Single source of truth for desktop (Rust/JS) and Supabase edge functions (Zod).

## Why

Drift between **heartbeat** and **claim-batch** caused HTTP 400 (`business_id` missing on claim while heartbeat succeeded). Contract validation catches that **before** network calls and in diagnostics as `contract validation failed: …` instead of opaque 400s.

## Artifacts

| Contract | JSON Schema | Lovable Zod | Desktop validator |
|----------|-------------|-------------|-------------------|
| `ccc-package-claim-batch` (request) | `docs/contracts/ccc-package-claim-batch.schema.json` | `backend/contracts/cccPackageClaimBatch.ts` | `api_contracts::validate_claim_batch_request` |
| `ccc-package-claim-batch` (response item) | `docs/contracts/ccc-package-claim-batch-item.schema.json` | `CccPackageClaimItemSchema` in same TS file | `destination_path_for_item` in `ccc_package_sync.rs` |
| `uce-ingest` heartbeat | `docs/contracts/uce-ingest-heartbeat.schema.json` | `backend/contracts/uceIngestHeartbeat.ts` | `api_contracts::validate_heartbeat_request` |
| `ccc-package-ack` | `docs/contracts/ccc-package-ack.schema.json` | `backend/contracts/cccPackageAck.ts` | `api_contracts::validate_package_ack_request` |

## Desktop behavior

- **Claim poll:** validates `business_id` (UUID), `device_id`, `limit` → sets `last_ccc_claim_error` with `contract validation failed: …` and **does not POST** if invalid.
- **Ack:** validates before each ack POST.
- **Connection test heartbeat:** validates before POST.

## Edge behavior (Lovable)

Use the Zod schemas in `backend/contracts/`. On failure return **400**:

```json
{
  "error": "contract_validation_failed",
  "missing_fields": ["business_id"],
  "details": "…"
}
```

Parse with the same schema on success paths.

## Changing a contract

1. Update JSON Schema in `docs/contracts/`.
2. Update `backend/contracts/*.ts` (Zod).
3. Update `src-tauri/src/api_contracts.rs`.
4. Bump agent + edge deploy together (same release train).
