import { z } from "zod";

/** POST /functions/v1/ccc-package-ack */
export const CccPackageAckRequestSchema = z.object({
  queue_id: z.string().uuid(),
  source_table: z.string().min(1),
  source_id: z.string().min(1),
  status: z.enum(["ok", "error"]),
  error_message: z.string().nullable().optional(),
  /** Optional echo from desktop when mirror used sub_folder layout */
  sub_folder: z.string().min(1).optional(),
  /** Echo of job kind (mirror_file, delete_folder, …) */
  action_type: z
    .enum(["mirror_file", "delete_folder", "delete_file", "archive_folder"])
    .optional(),
});

export type CccPackageAckRequest = z.infer<typeof CccPackageAckRequestSchema>;

export function parsePackageAckRequest(body: unknown) {
  const r = CccPackageAckRequestSchema.safeParse(body);
  if (r.success) {
    return { ok: true as const, data: r.data };
  }
  return {
    ok: false as const,
    error: "contract_validation_failed",
    missing_fields: r.error.issues.map((i) => i.path.join(".") || "body"),
  };
}
