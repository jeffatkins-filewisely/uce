import { z } from "zod";

/** POST uce-ingest (or uce-flag-pay) when action === "flag_pay_sync" */
export const UceFlagPayLaborLineSchema = z.object({
  op: z.string().min(1).max(64),
  hours: z.number().min(0.1).max(40),
  tech: z.string().optional().default(""),
});

export const UceFlagPaySyncRequestSchema = z.object({
  action: z.literal("flag_pay_sync"),
  business_id: z.string().uuid(),
  device_id: z.string().min(8).max(128),
  repair_order_number: z.string().min(4).max(16),
  ro_number: z.string().min(4).max(16).optional(),
  source_system: z.enum(["ccc", "mitchell", "unknown"]).or(z.string()),
  document_type: z.literal("work_order").or(z.string()).optional(),
  window_title: z.string().optional(),
  file_path: z.string().optional(),
  flag_hours_total: z.number().min(0.1).max(80),
  labor_lines: z.array(UceFlagPayLaborLineSchema).optional().default([]),
  update_ro_repair_flags: z.boolean().optional().default(true),
  evidence: z.array(z.string()).optional(),
});

export type UceFlagPaySyncRequest = z.infer<typeof UceFlagPaySyncRequestSchema>;

export function parseFlagPaySyncRequest(body: unknown) {
  const r = UceFlagPaySyncRequestSchema.safeParse(body);
  if (r.success) {
    return { ok: true as const, data: r.data };
  }
  return {
    ok: false as const,
    error: "contract_validation_failed",
    missing_fields: r.error.issues.map((i) => i.path.join(".") || "body"),
    details: r.error.flatten(),
  };
}
