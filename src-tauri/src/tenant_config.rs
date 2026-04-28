//! Per-machine tenant: `business_id`, and optional `backend_url` + `anon_key` (e.g. from
//! `uce://connect?...` or installer). Vite env vars are fallbacks in the webview.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::Manager;

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct TenantConfig {
    #[serde(default)]
    pub business_id: String,
    /// Ingest (uce-ingest) URL — same as `VITE_UCE_UPLOAD_URL` when not baked in.
    #[serde(default)]
    pub backend_url: String,
    /// Supabase anon (Bearer + apikey) when not in build env.
    #[serde(default)]
    pub anon_key: String,
}

fn tenant_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir error: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create app data dir error: {e}"))?;
    Ok(dir.join("uce-tenant.json"))
}

/// Strip UTF-8 BOM (Notepad “UTF-8” often writes one) so `serde_json` can parse.
fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    if bytes.len() >= 3 && bytes[0..3] == [0xef, 0xbb, 0xbf] {
        &bytes[3..]
    } else {
        bytes
    }
}

fn read_config(app: &tauri::AppHandle) -> Result<TenantConfig, String> {
    let path = tenant_file(app)?;
    if !path.exists() {
        return Ok(TenantConfig::default());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let slice = strip_utf8_bom(&bytes);
    let raw = match std::str::from_utf8(slice) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "[UCE] uce-tenant.json: invalid UTF-8: {e} — using empty tenant until the file is fixed"
            );
            return Ok(TenantConfig::default());
        }
    };
    match serde_json::from_str::<TenantConfig>(raw) {
        Ok(cfg) => Ok(cfg),
        Err(e) => {
            eprintln!(
                "[UCE] uce-tenant.json: JSON parse error: {e} — fix the file (valid JSON, no // comments) or delete it; using empty tenant for now"
            );
            Ok(TenantConfig::default())
        }
    }
}

fn write_config(app: &tauri::AppHandle, cfg: &TenantConfig) -> Result<(), String> {
    let path = tenant_file(app)?;
    let raw = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| e.to_string())
}

/// Full tenant file contents (defaults if missing).
pub fn load_tenant_config(app: &tauri::AppHandle) -> Result<TenantConfig, String> {
    read_config(app)
}

pub fn load_tenant_business_id(app: &tauri::AppHandle) -> Result<Option<String>, String> {
    let cfg = read_config(app)?;
    let id = cfg.business_id.trim();
    if id.is_empty() {
        Ok(None)
    } else {
        Ok(Some(id.to_string()))
    }
}

pub fn save_tenant_business_id(app: &tauri::AppHandle, business_id: String) -> Result<(), String> {
    let id = business_id.trim();
    if id.is_empty() {
        return Err("business_id is empty".to_string());
    }
    let mut cfg = read_config(app)?;
    cfg.business_id = id.to_string();
    write_config(app, &cfg)
}

/// `uce://connect` (or first launch) — set `business_id` and optional ingest credentials.
/// `None` for `backend_url` / `anon_key` leaves those fields unchanged on disk.
/// Replace all tenant fields at once (manual Connection Doctor form).
pub fn save_tenant_manual_all(
    app: &tauri::AppHandle,
    business_id: String,
    backend_url: String,
    anon_key: String,
) -> Result<(), String> {
    let id = business_id.trim();
    if id.is_empty() {
        return Err("business_id is empty".to_string());
    }
    let cfg = TenantConfig {
        business_id: id.to_string(),
        backend_url: backend_url.trim().to_string(),
        anon_key: anon_key.trim().to_string(),
    };
    write_config(app, &cfg)?;
    eprintln!(
        "UCE_TENANT_CONFIG_SAVED source=manual_all business_id_len={} backend_len={} anon_len={}",
        cfg.business_id.len(),
        cfg.backend_url.len(),
        cfg.anon_key.len()
    );
    eprintln!(
        "[UCE] uce-tenant.json written (manual) backend_len={} anon_len={}",
        cfg.backend_url.len(),
        cfg.anon_key.len()
    );
    Ok(())
}

pub fn tenant_config_path_for_display(app: &tauri::AppHandle) -> Result<String, String> {
    let p = tenant_file(app)?;
    Ok(p.to_string_lossy().into_owned())
}

pub fn save_tenant_from_connect(
    app: &tauri::AppHandle,
    business_id: String,
    backend_url: Option<String>,
    anon_key: Option<String>,
) -> Result<(), String> {
    let id = business_id.trim();
    if id.is_empty() {
        return Err("business_id is empty".to_string());
    }
    let mut cfg = read_config(app)?;
    cfg.business_id = id.to_string();
    if let Some(ref u) = backend_url {
        let t = u.trim();
        if !t.is_empty() {
            cfg.backend_url = t.to_string();
        }
    }
    if let Some(ref k) = anon_key {
        let t = k.trim();
        if !t.is_empty() {
            cfg.anon_key = t.to_string();
        }
    }
    write_config(app, &cfg)?;
    eprintln!(
        "[UCE] uce-tenant.json written (from_connect) backend_len={} anon_set={}",
        cfg.backend_url.len(),
        !cfg.anon_key.is_empty()
    );
    Ok(())
}
