//! Stable device id shared with the webview (`uceDeviceId.js` via `uce_sync_device_id`).

use std::fs;
use std::path::PathBuf;
use tauri::AppHandle;
use tauri::Manager;

const DEVICE_ID_FILE: &str = "uce-device-id.txt";

fn device_id_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("create app data dir: {e}"))?;
    Ok(dir.join(DEVICE_ID_FILE))
}

pub fn load_device_id(app: &AppHandle) -> Option<String> {
    let path = device_id_path(app).ok()?;
    if !path.exists() {
        return None;
    }
    let raw = fs::read_to_string(&path).ok()?;
    let id = raw.trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

pub fn save_device_id(app: &AppHandle, device_id: &str) -> Result<(), String> {
    let id = device_id.trim();
    if id.is_empty() {
        return Err("device_id is empty".to_string());
    }
    let path = device_id_path(app)?;
    fs::write(path, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn uce_sync_device_id(app: AppHandle, device_id: String) -> Result<(), String> {
    save_device_id(&app, &device_id)
}

#[tauri::command]
pub fn uce_get_device_id(app: AppHandle) -> Result<String, String> {
    load_device_id(&app).ok_or_else(|| "device_id not set".to_string())
}
