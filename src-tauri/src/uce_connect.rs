//! Parse `uce://connect` (and FileWisely https landing URLs) and persist tenant
//! credentials. Runs in Rust so pairing works even if the webview JS is not
//! ready yet — the previous JS-only path missed first-launch Connect clicks.

use crate::api_contracts::is_uuid;
use crate::filewisely_defaults;
use crate::tenant_config;
use serde::Deserialize;
use tauri::{AppHandle, Emitter};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConnectParams {
    pub business_id: Option<String>,
    pub backend_url: Option<String>,
    pub anon_key: Option<String>,
    pub handshake_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HandshakeClaimResponse {
    business_id: Option<String>,
    backend_url: Option<String>,
    anon_key: Option<String>,
}

fn first_nonempty(u: &Url, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(v) = u.query_pairs().find(|(qk, _)| qk.eq_ignore_ascii_case(k)) {
            let t = v.1.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn decode_if_encoded(raw: &str) -> String {
    let s = raw.trim();
    // Only decode when the scheme itself is escaped (Windows sometimes passes
    // `uce%3A%2F%2Fconnect?...`). Leave query encoding for Url::query_pairs.
    if s.to_ascii_lowercase().starts_with("uce%3a") {
        match urlencoding_loose(s) {
            Some(d) if !d.is_empty() => d,
            _ => s.to_string(),
        }
    } else {
        s.to_string()
    }
}

fn urlencoding_loose(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

fn normalize_to_http_url(raw: &str) -> Option<String> {
    let s = decode_if_encoded(raw);
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.to_ascii_lowercase().starts_with("uce:") {
        let rest = s.get(4..).unwrap_or("");
        let rest = rest.trim_start_matches('/').trim_start_matches('/');
        return Some(format!("http://uce.invalid/{rest}"));
    }
    if s.to_ascii_lowercase().starts_with("http://") || s.to_ascii_lowercase().starts_with("https://")
    {
        return Some(s.to_string());
    }
    None
}

fn looks_like_filewisely_connect_path(path: &str) -> bool {
    let p = path.trim_matches('/').to_ascii_lowercase();
    p.is_empty()
        || p == "connect"
        || p.contains("uce-connect")
        || p.contains("connect-uce")
        || p.ends_with("/connect")
        || p.contains("uce/connect")
}

/// Parse `uce://connect?...` or an https FileWisely landing URL with the same query.
pub fn parse_connect_url(raw: &str) -> Option<ConnectParams> {
    let normalized = normalize_to_http_url(raw)?;
    let u = Url::parse(&normalized).ok()?;
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    let is_uce_scheme = host == "uce.invalid" || host == "connect" || host.is_empty();
    let is_filewisely_https = host.contains("filewisely") || host.contains("supabase.co");
    if !is_uce_scheme && !is_filewisely_https && !looks_like_filewisely_connect_path(u.path()) {
        return None;
    }
    if is_uce_scheme && !looks_like_filewisely_connect_path(u.path()) {
        return None;
    }

    let handshake_explicit = first_nonempty(&u, &["handshake_token", "claim_token"]);
    let token_or_code = first_nonempty(&u, &["token", "code"]);
    let business = first_nonempty(&u, &["business_id", "businessId", "shop_id"]);
    let backend = first_nonempty(
        &u,
        &["backend_url", "ingest_url", "upload_url", "backendUrl"],
    );
    let anon = first_nonempty(
        &u,
        &[
            "anon_key",
            "api_key",
            "apikey",
            "supabase_anon_key",
            "anonKey",
        ],
    );

    let mut handshake = handshake_explicit;
    let mut business_id = business;
    if let Some(t) = token_or_code {
        if is_uuid(&t) {
            if business_id.is_none() {
                business_id = Some(t);
            }
        } else if handshake.is_none() {
            handshake = Some(t);
        }
    }

    if handshake.is_none() && business_id.is_none() {
        return None;
    }

    Some(ConnectParams {
        business_id,
        backend_url: backend,
        anon_key: anon,
        handshake_token: handshake,
    })
}

async fn claim_handshake(token: &str, backend_hint: &str, anon_hint: &str) -> Result<HandshakeClaimResponse, String> {
    let url = filewisely_defaults::handshake_claim_url(backend_hint);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "token": token }));
    let key = anon_hint.trim();
    if key.is_empty() {
        let baked = filewisely_defaults::production_anon_key();
        if !baked.is_empty() {
            req = req
                .header("Authorization", format!("Bearer {baked}"))
                .header("apikey", baked);
        }
    } else {
        req = req
            .header("Authorization", format!("Bearer {key}"))
            .header("apikey", key);
    }
    eprintln!("[UCE] handshake claim POST {url}");
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "handshake HTTP {} {}",
            status.as_u16(),
            text.chars().take(240).collect::<String>()
        ));
    }
    serde_json::from_str::<HandshakeClaimResponse>(&text)
        .or_else(|_| {
            // Some deployments wrap the payload.
            #[derive(Deserialize)]
            struct Wrap {
                business_id: Option<String>,
                backend_url: Option<String>,
                anon_key: Option<String>,
            }
            serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| {
                    let inner = v.get("data").cloned().unwrap_or(v);
                    serde_json::from_value::<Wrap>(inner).ok()
                })
                .map(|w| HandshakeClaimResponse {
                    business_id: w.business_id,
                    backend_url: w.backend_url,
                    anon_key: w.anon_key,
                })
                .ok_or_else(|| format!("handshake JSON parse failed: {}", text.chars().take(180).collect::<String>()))
        })
}

pub async fn apply_connect_urls(app: &AppHandle, urls: &[String]) -> bool {
    let mut applied = false;
    for raw in urls {
        let Some(parsed) = parse_connect_url(raw) else {
            if raw.to_ascii_lowercase().contains("uce:") {
                eprintln!(
                    "[UCE] rust connect: rejected URL (need connect + business_id and/or handshake_token) len={}",
                    raw.len()
                );
            }
            continue;
        };

        if let Some(ref token) = parsed.handshake_token {
            let hint = parsed.backend_url.clone().unwrap_or_default();
            let anon_hint = parsed.anon_key.clone().unwrap_or_default();
            match claim_handshake(token, &hint, &anon_hint).await {
                Ok(data) => {
                    let bid = data
                        .business_id
                        .as_deref()
                        .or(parsed.business_id.as_deref())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !is_uuid(&bid) {
                        eprintln!("[UCE] rust handshake: missing or invalid business_id");
                        continue;
                    }
                    if let Err(e) = tenant_config::save_tenant_from_connect(
                        app,
                        bid.clone(),
                        data.backend_url.or(parsed.backend_url.clone()),
                        data.anon_key.or(parsed.anon_key.clone()),
                    ) {
                        eprintln!("[UCE] rust handshake save: {e}");
                        continue;
                    }
                    eprintln!("UCE_HANDSHAKE_CLAIM_OK source=rust business_id={}", &bid[..8.min(bid.len())]);
                    applied = true;
                    break;
                }
                Err(e) => {
                    eprintln!("[UCE] rust handshake claim error: {e}");
                    // Fall through: a UUID business_id on the same link can still pair.
                }
            }
        }

        if let Some(ref bid) = parsed.business_id {
            if !is_uuid(bid) {
                eprintln!(
                    "[UCE] rust connect: business_id is not a UUID — prefix={}",
                    bid.chars().take(8).collect::<String>()
                );
                continue;
            }
            if let Err(e) = tenant_config::save_tenant_from_connect(
                app,
                bid.clone(),
                parsed.backend_url.clone(),
                parsed.anon_key.clone(),
            ) {
                eprintln!("[UCE] rust connect save: {e}");
                continue;
            }
            eprintln!("[UCE] rust connect: uce-tenant.json saved from deep link");
            applied = true;
            break;
        }
    }

    if applied {
        if let Err(e) = app.emit("uce-tenant-saved", ()) {
            eprintln!("[UCE] emit uce-tenant-saved after connect: {e}");
        }
    }
    applied
}

pub fn spawn_apply_connect_urls(app: AppHandle, urls: Vec<String>) {
    if urls.is_empty() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        apply_connect_urls(&app, &urls).await;
    });
}

fn argv_deeplinks() -> Vec<String> {
    std::env::args()
        .filter(|a| {
            let s = a.to_ascii_lowercase();
            s.contains("uce:") || s.contains("uce%3a")
        })
        .collect()
}

/// Register plugin listener + apply any URL that launched this process.
pub fn listen_and_apply_startup_deeplinks(app: &AppHandle) {
    use tauri_plugin_deep_link::DeepLinkExt;

    let from_argv = argv_deeplinks();
    if !from_argv.is_empty() {
        eprintln!("[UCE] rust connect argv: {} URL(s)", from_argv.len());
        spawn_apply_connect_urls(app.clone(), from_argv);
    }

    match app.deep_link().get_current() {
        Ok(Some(urls)) => {
            let raw: Vec<String> = urls.iter().map(|u| u.to_string()).collect();
            if !raw.is_empty() {
                eprintln!("[UCE] rust deep link get_current: {} URL(s)", raw.len());
                spawn_apply_connect_urls(app.clone(), raw);
            }
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("[UCE] rust deep link get_current: {e}");
        }
    }

    let handle = app.clone();
    app.deep_link().on_open_url(move |event| {
        let raw: Vec<String> = event.urls().iter().map(|u| u.to_string()).collect();
        if raw.is_empty() {
            return;
        }
        eprintln!("[UCE] rust deep link on_open_url: {} URL(s)", raw.len());
        spawn_apply_connect_urls(handle.clone(), raw);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_business_id_only() {
        let p = parse_connect_url(
            "uce://connect?business_id=550e8400-e29b-41d4-a716-446655440000",
        )
        .expect("parse");
        assert_eq!(
            p.business_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert!(p.handshake_token.is_none());
    }

    #[test]
    fn treats_non_uuid_token_as_handshake() {
        let p = parse_connect_url("uce://connect?token=shop-pair-abc123").expect("parse");
        assert_eq!(p.handshake_token.as_deref(), Some("shop-pair-abc123"));
        assert!(p.business_id.is_none());
    }

    #[test]
    fn treats_uuid_token_as_business_id() {
        let p = parse_connect_url(
            "uce://connect?token=550e8400-e29b-41d4-a716-446655440000",
        )
        .expect("parse");
        assert_eq!(
            p.business_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert!(p.handshake_token.is_none());
    }

    #[test]
    fn accepts_single_slash_and_encoded() {
        let p = parse_connect_url(
            "uce:connect?handshake_token=abc&backend_url=https%3A%2F%2Fexample.test%2Fuce-ingest",
        )
        .expect("parse");
        assert_eq!(p.handshake_token.as_deref(), Some("abc"));
        assert_eq!(
            p.backend_url.as_deref(),
            Some("https://example.test/uce-ingest")
        );
    }

    #[test]
    fn accepts_three_codes_on_link() {
        let p = parse_connect_url(
            "uce://connect?business_id=550e8400-e29b-41d4-a716-446655440000&backend_url=https://pujwbzqnoevqxrwipnwo.supabase.co/functions/v1/uce-ingest&anon_key=eyJtest",
        )
        .expect("parse");
        assert!(p.business_id.is_some());
        assert!(p.backend_url.unwrap().contains("uce-ingest"));
        assert_eq!(p.anon_key.as_deref(), Some("eyJtest"));
    }

    #[test]
    fn rejects_unrelated_uce_path() {
        assert!(parse_connect_url("uce://status?business_id=550e8400-e29b-41d4-a716-446655440000").is_none());
    }
}
