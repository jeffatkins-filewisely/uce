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

/// Production Supabase anon / publishable JWT (role=anon, project pujwbzqnoevqxrwipnwo).
/// Same value FileWisely ships as `VITE_SUPABASE_PUBLISHABLE_KEY`. Public-by-design (RLS).
pub const PRODUCTION_ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6InB1andienFub2V2cXhyd2lwbndvIiwicm9sZSI6ImFub24iLCJpYXQiOjE3NjE1NjQ3OTYsImV4cCI6MjA3NzE0MDc5Nn0.4FmMOBHVye4ewriLnmcwR_JhmlnL4hIE3j51LL7xM7k";

/// Compile-time override (`UCE_PRODUCTION_ANON_KEY` / Vite env on MSI) wins when set;
/// otherwise the baked production key is used so shops never paste it.
pub fn production_anon_key() -> String {
    let from_env = option_env!("UCE_PRODUCTION_ANON_KEY")
        .or_else(|| option_env!("VITE_UCE_SUPABASE_ANON_KEY"))
        .or_else(|| option_env!("VITE_SUPABASE_ANON_KEY"))
        .unwrap_or("")
        .trim();
    if from_env.is_empty() {
        PRODUCTION_ANON_KEY.to_string()
    } else {
        from_env.to_string()
    }
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

    #[test]
    fn production_anon_key_is_baked_in() {
        let key = production_anon_key();
        assert!(key.starts_with("eyJ"));
        assert!(key.len() > 80);
        assert_eq!(key, PRODUCTION_ANON_KEY);
    }
}
