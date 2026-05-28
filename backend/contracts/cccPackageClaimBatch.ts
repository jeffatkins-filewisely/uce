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

const claimItemBase = z.object({
  queue_id: z.string().uuid(),
  action_type: z
    .enum(["mirror_file", "delete_folder", "delete_file", "archive_folder"])
    .optional(),
  source_table: z.string().min(1),
  source_id: z.string().min(1),
});

const mirrorFileFields = z.object({
  ro_folder: z.string().min(1).optional(),
  ro_folder_name: z.string().min(1).optional(),
  sub_folder: z.string().min(1).optional(),
  bucket: z.string().optional(),
  filename_hint: z.string().min(1).optional(),
  filename: z.string().min(1).optional(),
  signed_url: z.string().url(),
});

const deleteFolderFields = z.object({
  action_type: z.literal("delete_folder"),
  target_path_hint: z.string().min(1),
});

/** One job from claim-batch response `items[]` */
export const CccPackageClaimItemSchema = z.union([
  claimItemBase.merge(deleteFolderFields),
  claimItemBase.merge(mirrorFileFields).refine(
    (i) => !!(i.ro_folder || i.ro_folder_name),
    { message: "ro_folder or ro_folder_name required" },
  ).refine(
    (i) => !!(i.filename_hint || i.filename),
    { message: "filename_hint or filename required" },
  ),
  claimItemBase.merge(mirrorFileFields).extend({
    action_type: z.literal("mirror_file"),
  }).refine(
    (i) => !!(i.ro_folder || i.ro_folder_name),
    { message: "ro_folder or ro_folder_name required" },
  ).refine(
    (i) => !!(i.filename_hint || i.filename),
    { message: "filename_hint or filename required" },
  ),
]);

export type CccPackageClaimItem = z.infer<typeof CccPackageClaimItemSchema>;

export const CccPackageClaimBatchResponseSchema = z.object({
  items: z.array(CccPackageClaimItemSchema).default([]),
});

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
