//! Production FileWisely endpoints shared by every shop MSI.
//!
//! Per-shop identity is only `business_id`. Ingest URL (and handshake URL) are
//! the same for Anaheim and every other customer. The release workflow used to
//! omit `VITE_UCE_UPLOAD_URL`, so Connect-to-computer / `uce://` links that
//! carried only a business id or handshake token could never finish pairing.

/// Production Supabase project (see `docs/REPO_WORKFLOW.md`).
pub const PRODUCTION_PROJECT_REF: &str = "pujwbzqnoevqxrwipnwo";

pub const PRODUCTION_INGEST_URL: &str =
    "https://pujwbzqnoevqxrwipnwo.supabase.co/functions/v1/uce-ingest";

pub const PRODUCTION_HANDSHAKE_URL: &str =
    "https://pujwbzqnoevqxrwipnwo.supabase.co/functions/v1/uce-claim-handshake";

/// Optional compile-time anon key (`UCE_PRODUCTION_ANON_KEY` / Vite env on MSI).
/// Supabase anon keys are public-by-design (RLS); shops should not have to paste them.
pub fn production_anon_key() -> String {
    option_env!("UCE_PRODUCTION_ANON_KEY")
        .or_else(|| option_env!("VITE_UCE_SUPABASE_ANON_KEY"))
        .or_else(|| option_env!("VITE_SUPABASE_ANON_KEY"))
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Fill empty ingest URL / anon key with production defaults. Never overwrites
/// a value already stored on disk (so a custom backend still wins).
pub fn apply_production_defaults(cfg: &mut crate::tenant_config::TenantConfig) {
    if cfg.backend_url.trim().is_empty() {
        cfg.backend_url = PRODUCTION_INGEST_URL.to_string();
    }
    if cfg.anon_key.trim().is_empty() {
        let key = production_anon_key();
        if !key.is_empty() {
            cfg.anon_key = key;
        }
    }
}

pub fn handshake_claim_url(backend_url_hint: &str) -> String {
    let hint = backend_url_hint.trim();
    if !hint.is_empty() {
        if let Ok(mut u) = url::Url::parse(hint) {
            let path = u.path().replace("uce-ingest", "uce-claim-handshake");
            u.set_path(&path);
            return u.to_string();
        }
    }
    PRODUCTION_HANDSHAKE_URL.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_url_from_ingest() {
        let got = handshake_claim_url(PRODUCTION_INGEST_URL);
        assert!(got.contains("uce-claim-handshake"));
        assert!(!got.contains("uce-ingest"));
    }

    #[test]
    fn handshake_url_empty_uses_production() {
        assert_eq!(handshake_claim_url(""), PRODUCTION_HANDSHAKE_URL);
    }
}
