//! Fetches org-wide watch rules from Filewisely (or any HTTPS JSON endpoint) and replaces local
//! `uce-custom-known-rules.json` + `uce-exclude-rules.json` so every PC matches one policy.
//!
//! ## Filewisely HTTP contract
//!
//! - **GET** the configured URL. `business_id` is appended as a query param unless the URL already
//!   contains `business_id=`, or use the literal `{business_id}` in the path.
//! - Optional header: `Authorization: Bearer <token>` when `authorization` is set (e.g. Supabase anon).
//! - **200** body, `application/json`:
//! ```json
//! {
//!   "version": "shop-policy-v3",
//!   "trained": [
//!     {
//!       "id": "ccc_trained_remote_1",
//!       "app_keywords": ["chrome", "msedge", "firefox"],
//!       "title_keywords": ["repair order"],
//!       "priority": 5,
//!       "cooldown_secs": 8
//!     }
//!   ],
//!   "excluded": [
//!     {
//!       "id": "ccc_exclude_remote_1",
//!       "app_keywords": ["chrome"],
//!       "title_keywords": ["facebook"],
//!       "priority": 5,
//!       "cooldown_secs": 0
//!     }
//!   ]
//! }
//! ```
//! - Field aliases accepted: `trainedRules` → `trained`, `excludedRules` / `excludes` → `excluded`.
//! - A successful sync **overwrites** both local rule files. Per-machine “Train (T)” edits are replaced
//!   on the next sync unless you merge them server-side.

use crate::memory_store::{load_memory, save_custom_rules, save_exclude_rules, save_memory};
use crate::types::Rule;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchPolicyDocument {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default, alias = "trainedRules")]
    pub trained: Vec<Rule>,
    #[serde(default, alias = "excludedRules", alias = "excludes")]
    pub excluded: Vec<Rule>,
}

pub fn apply_watch_policy_to_disk(app: &AppHandle, doc: &WatchPolicyDocument) -> Result<(), String> {
    save_custom_rules(app, &doc.trained)?;
    save_exclude_rules(app, &doc.excluded)?;

    let mut memory = load_memory(app)?;
    if let Some(v) = &doc.version {
        memory.approved_contexts_version = Some(v.clone());
    }
    memory.approved_contexts_last_loaded_unix_ms = Some(now_unix_ms());
    save_memory(app, &memory)?;
    Ok(())
}

fn build_policy_url(base_url: &str, business_id: &str) -> Result<String, String> {
    let base = base_url.trim();
    if base.is_empty() {
        return Err("empty policy URL".to_string());
    }
    if base.contains("{business_id}") {
        return Ok(base.replace("{business_id}", business_id));
    }
    let mut u = reqwest::Url::parse(base).map_err(|e| format!("invalid policy URL: {e}"))?;
    let has_business = u
        .query_pairs()
        .any(|(k, _)| k.as_ref() == "business_id");
    if !has_business {
        u.query_pairs_mut()
            .append_pair("business_id", business_id);
    }
    Ok(u.to_string())
}

pub async fn fetch_and_apply_watch_policy(
    app: &AppHandle,
    base_url: &str,
    business_id: &str,
    authorization: Option<&str>,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;

    let url = build_policy_url(base_url, business_id)?;
    let mut req = client.get(&url);
    if let Some(token) = authorization {
        let t = token.trim();
        if !t.is_empty() {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
    }
    let resp = req
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("watch policy request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "watch policy HTTP {} — {}",
            status,
            body.chars().take(200).collect::<String>()
        ));
    }

    let doc: WatchPolicyDocument = resp
        .json()
        .await
        .map_err(|e| format!("watch policy JSON: {e}"))?;

    apply_watch_policy_to_disk(app, &doc)?;

    let ver = doc
        .version
        .clone()
        .unwrap_or_else(|| "(no version)".to_string());
    Ok(format!("Watch policy synced: {ver}"))
}
