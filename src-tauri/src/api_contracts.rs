//! Client-side contract validation before edge function calls.
//! Schemas: `docs/contracts/*.schema.json` — mirror in Lovable with `backend/contracts/*.ts`.

const CLAIM_LIMIT_MIN: u32 = 1;
const CLAIM_LIMIT_MAX: u32 = 100;

/// RFC 4122-style UUID (8-4-4-4-12 hex), case-insensitive.
pub fn is_uuid(s: &str) -> bool {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (i, &c) in b.iter().enumerate() {
        let dash = i == 8 || i == 13 || i == 18 || i == 23;
        if dash {
            if c != b'-' {
                return false;
            }
        } else if !c.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn device_id_ok(s: &str) -> bool {
    let t = s.trim();
    t.len() >= 8 && t.len() <= 128 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `ccc-package-claim-batch` POST body — must match edge Zod + JSON Schema.
pub fn validate_claim_batch_request(
    business_id: &str,
    device_id: &str,
    limit: u32,
) -> Result<(), String> {
    let mut problems: Vec<String> = Vec::new();

    let bid = business_id.trim();
    if bid.is_empty() {
        problems.push("business_id is required".to_string());
    } else if !is_uuid(bid) {
        problems.push(format!(
            "business_id must be a UUID (got {:?})",
            bid.chars().take(40).collect::<String>()
        ));
    }

    let did = device_id.trim();
    if did.is_empty() {
        problems.push("device_id is required".to_string());
    } else if !device_id_ok(did) {
        problems.push("device_id must be 8–128 chars (alphanumeric, -, _)".to_string());
    }

    if limit < CLAIM_LIMIT_MIN || limit > CLAIM_LIMIT_MAX {
        problems.push(format!(
            "limit must be between {CLAIM_LIMIT_MIN} and {CLAIM_LIMIT_MAX} (got {limit})"
        ));
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("contract validation failed: {}", problems.join("; ")))
    }
}

/// `uce-ingest` heartbeat (`action: "heartbeat"`) — required identity fields.
pub fn validate_heartbeat_request(business_id: &str, device_id: &str) -> Result<(), String> {
    let mut problems: Vec<String> = Vec::new();

    let bid = business_id.trim();
    if bid.is_empty() {
        problems.push("business_id is required".to_string());
    } else if !is_uuid(bid) {
        problems.push("business_id must be a UUID".to_string());
    }

    let did = device_id.trim();
    if did.is_empty() {
        problems.push("device_id is required".to_string());
    } else if !device_id_ok(did) {
        problems.push("device_id invalid".to_string());
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("contract validation failed: {}", problems.join("; ")))
    }
}

/// `ccc-package-ack` POST body.
pub fn validate_package_ack_request(
    queue_id: &str,
    status: &str,
    source_table: &str,
    source_id: &str,
) -> Result<(), String> {
    let mut problems: Vec<String> = Vec::new();

    let qid = queue_id.trim();
    if qid.is_empty() {
        problems.push("queue_id is required".to_string());
    } else if !is_uuid(qid) {
        problems.push(format!(
            "queue_id must be a UUID (got {:?})",
            qid.chars().take(40).collect::<String>()
        ));
    }
    if source_table.trim().is_empty() {
        problems.push("source_table is required".to_string());
    }
    if source_id.trim().is_empty() {
        problems.push("source_id is required".to_string());
    }
    match status.trim() {
        "ok" | "error" => {}
        other => problems.push(format!("status must be ok or error (got {other:?})")),
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("contract validation failed: {}", problems.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_accepts_valid() {
        assert!(is_uuid("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn claim_requires_business_id() {
        assert!(validate_claim_batch_request("", "uce-abc", 25).is_err());
    }
}
