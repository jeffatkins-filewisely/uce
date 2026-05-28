import { z } from "zod";

const deviceId = z.string().min(8).max(128).regex(/^[A-Za-z0-9_-]+$/);

/** One element of `ccc-package-ack` `items[]` */
export const CccPackageAckItemSchema = z.object({
  queue_id: z.string().uuid(),
  source_table: z.string().min(1),
  source_id: z.string().min(1),
  status: z.enum(["ok", "error"]),
  written_path: z.string().min(1).optional(),
  error_message: z.string().nullable().optional(),
  sub_folder: z.string().min(1).optional(),
  action_type: z
    .enum(["mirror_file", "delete_folder", "delete_file", "archive_folder"])
    .optional(),
});

export type CccPackageAckItem = z.infer<typeof CccPackageAckItemSchema>;

/** POST /functions/v1/ccc-package-ack — canonical batch body (Sidekick v0.1.65+) */
export const CccPackageAckBatchRequestSchema = z.object({
  business_id: z.string().uuid(),
  device_id: deviceId,
  items: z.array(CccPackageAckItemSchema).min(1),
});

export type CccPackageAckBatchRequest = z.infer<typeof CccPackageAckBatchRequestSchema>;

/** @deprecated Single-item flat body; edge v2 shim accepts legacy shapes */
export const CccPackageAckRequestSchema = CccPackageAckItemSchema;

export type CccPackageAckRequest = z.infer<typeof CccPackageAckRequestSchema>;

export function parsePackageAckBatchRequest(body: unknown) {
  const r = CccPackageAckBatchRequestSchema.safeParse(body);
  if (r.success) {
    return { ok: true as const, data: r.data };
  }
  return {
    ok: false as const,
    error: "contract_validation_failed",
    missing_fields: r.error.issues.map((i) => i.path.join(".") || "body"),
  };
}

export function parsePackageAckRequest(body: unknown) {
  return parsePackageAckBatchRequest(body);
}
