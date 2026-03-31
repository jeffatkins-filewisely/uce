//! One **business_id** (Filewisely tenant UUID) per machine install.
//!
//! Resolution order in the frontend:
//! 1. `uce-tenant.json` in app data (set by installer, MDM, or `save_tenant_business_id`)
//! 2. `VITE_UCE_BUSINESS_ID` at build time (dev / per-customer branded builds)
//!
//! Never hardcode a tenant in the binary — each shop must route to its own RLS scope on the backend.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::Manager;

#[derive(Serialize, Deserialize, Default)]
struct TenantConfig {
    #[serde(default)]
    business_id: String,
}

fn tenant_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir error: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create app data dir error: {e}"))?;
    Ok(dir.join("uce-tenant.json"))
}

pub fn load_tenant_business_id(app: &tauri::AppHandle) -> Result<Option<String>, String> {
    let path = tenant_file(app)?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let cfg: TenantConfig = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
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
    let path = tenant_file(app)?;
    let cfg = TenantConfig {
        business_id: id.to_string(),
    };
    let raw = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| e.to_string())
}
