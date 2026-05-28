import { z } from "zod";

/** POST /functions/v1/ccc-package-claim-batch — keep in sync with docs/contracts/ccc-package-claim-batch.schema.json */
export const CccPackageClaimBatchRequestSchema = z.object({
  business_id: z.string().uuid(),
  device_id: z.string().min(8).max(128).regex(/^[A-Za-z0-9_-]+$/),
  limit: z.number().int().min(1).max(100),
});

export type CccPackageClaimBatchRequest = z.infer<
  typeof CccPackageClaimBatchRequestSchema
>;

export function parseClaimBatchRequest(
  body: unknown,
):
  | { ok: true; data: CccPackageClaimBatchRequest }
  | { ok: false; error: string; missing_fields: string[] } {
  const r = CccPackageClaimBatchRequestSchema.safeParse(body);
  if (r.success) {
    return { ok: true, data: r.data };
  }
  const missing_fields = r.error.issues
    .map((i) => i.path.join(".") || "body")
    .filter(Boolean);
  return {
    ok: false,
    error: "contract_validation_failed",
    missing_fields,
  };
}
