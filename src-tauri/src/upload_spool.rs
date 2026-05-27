//! Durable outbound upload queue — survives restarts; JS drains via `uce:spool-drain`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;

const SPOOL_FILE: &str = "uce-upload-spool.json";
const MAX_ATTEMPTS: u32 = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpoolEntry {
    pub id: String,
    pub file_path: String,
    pub fingerprint: String,
    pub source: String,
    pub enqueued_unix_ms: i64,
    pub attempts: u32,
    pub last_error: String,
}

static SPOOL: Mutex<Option<Vec<SpoolEntry>>> = Mutex::new(None);

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn spool_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("create app data dir: {e}"))?;
    Ok(dir.join(SPOOL_FILE))
}

fn load_from_disk(app: &AppHandle) -> Result<Vec<SpoolEntry>, String> {
    let path = spool_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&raw).map_err(|e| format!("parse spool: {e}"))
}

fn persist(app: &AppHandle, entries: &[SpoolEntry]) -> Result<(), String> {
    let path = spool_path(app)?;
    let json = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

fn with_spool<F, R>(app: &AppHandle, f: F) -> Result<R, String>
where
    F: FnOnce(&mut Vec<SpoolEntry>) -> Result<R, String>,
{
    let mut guard = SPOOL.lock().map_err(|e| e.to_string())?;
    if guard.is_none() {
        *guard = Some(load_from_disk(app)?);
    }
    let vec = guard.as_mut().ok_or_else(|| "spool unavailable".to_string())?;
    let out = f(vec)?;
    persist(app, vec)?;
    Ok(out)
}

pub fn pending_count(app: &AppHandle) -> u32 {
    with_spool(app, |v| Ok(v.len() as u32)).unwrap_or(0)
}

#[tauri::command]
pub fn uce_spool_enqueue(
    app: AppHandle,
    file_path: String,
    fingerprint: String,
    source: String,
) -> Result<u32, String> {
    let fp = fingerprint.trim();
    let path = file_path.trim();
    if fp.is_empty() || path.is_empty() {
        return Err("file_path and fingerprint required".to_string());
    }
    let count = with_spool(&app, |entries| {
        if entries.iter().any(|e| e.fingerprint == fp) {
            return Ok(entries.len() as u32);
        }
        entries.push(SpoolEntry {
            id: fp.to_string(),
            file_path: path.to_string(),
            fingerprint: fp.to_string(),
            source: if source.trim().is_empty() {
                "spool".to_string()
            } else {
                source.trim().to_string()
            },
            enqueued_unix_ms: now_unix_ms(),
            attempts: 0,
            last_error: String::new(),
        });
        eprintln!(
            "UCE_UPLOAD_SPOOL_ENQUEUE path={} fp={}",
            path, fp
        );
        sync_spool_stats(&app, entries.len() as u32);
        Ok(entries.len() as u32)
    })?;
    Ok(count)
}

fn sync_spool_stats(app: &AppHandle, count: u32) {
    crate::device_health::set_spool_pending(count);
    crate::device_health::refresh_tray(app);
}

#[tauri::command]
pub fn uce_spool_claim_batch(app: AppHandle, limit: u32) -> Result<Vec<SpoolEntry>, String> {
    let lim = limit.clamp(1, 25) as usize;
    with_spool(&app, |entries| {
        let batch: Vec<SpoolEntry> = entries.iter().take(lim).cloned().collect();
        Ok(batch)
    })
}

#[tauri::command]
pub fn uce_spool_ack(app: AppHandle, id: String) -> Result<u32, String> {
    let key = id.trim();
    with_spool(&app, |entries| {
        let before = entries.len();
        entries.retain(|e| e.id != key);
        if entries.len() < before {
            eprintln!("UCE_UPLOAD_SPOOL_ACK id={}", key);
            crate::device_health::note_upload_activity();
        }
        sync_spool_stats(&app, entries.len() as u32);
        Ok(entries.len() as u32)
    })
}

#[tauri::command]
pub fn uce_spool_fail(app: AppHandle, id: String, error: String) -> Result<u32, String> {
    let key = id.trim();
    let err = error.trim();
    with_spool(&app, |entries| {
        if let Some(e) = entries.iter_mut().find(|e| e.id == key) {
            e.attempts = e.attempts.saturating_add(1);
            e.last_error = if err.is_empty() {
                "unknown".to_string()
            } else {
                err.chars().take(500).collect()
            };
            eprintln!(
                "UCE_UPLOAD_SPOOL_FAIL id={} attempts={} err={}",
                key, e.attempts, e.last_error
            );
            crate::device_health::set_last_error(format!(
                "upload spool: {}",
                e.last_error
            ));
            if e.attempts >= MAX_ATTEMPTS {
                eprintln!("UCE_UPLOAD_SPOOL_DROP_MAX_ATTEMPTS id={}", key);
                entries.retain(|x| x.id != key);
            }
        }
        sync_spool_stats(&app, entries.len() as u32);
        Ok(entries.len() as u32)
    })
}

#[tauri::command]
pub fn uce_spool_pending_count(app: AppHandle) -> Result<u32, String> {
    Ok(pending_count(&app))
}

pub fn init_spool_from_disk(app: &AppHandle) {
    if let Ok(n) = with_spool(app, |e| Ok(e.len() as u32)) {
        crate::device_health::set_spool_pending(n);
    }
}

pub fn spawn_spool_drain_loop(app: AppHandle) {
    init_spool_from_disk(&app);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            let count = pending_count(&app);
            if count == 0 {
                continue;
            }
            if crate::ccc_package_sync::is_sync_paused() {
                continue;
            }
            eprintln!("UCE_UPLOAD_SPOOL_DRAIN_NUDGE pending={}", count);
            let _ = app.emit("uce:spool-drain", count);
        }
    });
}
