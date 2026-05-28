import { z } from "zod";

/** POST uce-ingest when action === "heartbeat" */
export const UceIngestHeartbeatRequestSchema = z.object({
  action: z.literal("heartbeat"),
  business_id: z.string().uuid(),
  device_id: z.string().min(8).max(128),
  device_name: z.string().optional(),
  agent_version: z.string().optional(),
  os_info: z.string().optional(),
  user_id: z.string().optional(),
  ccc_package_capable: z.boolean().optional(),
  ccc_package_root: z.string().optional(),
  device_health: z.record(z.unknown()).optional(),
});

export type UceIngestHeartbeatRequest = z.infer<
  typeof UceIngestHeartbeatRequestSchema
>;

export function parseHeartbeatRequest(body: unknown) {
  const r = UceIngestHeartbeatRequestSchema.safeParse(body);
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
