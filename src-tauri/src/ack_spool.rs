//! Durable CCC package ack retry queue — survives restarts when edge ack POST fails.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Manager;

const SPOOL_FILE: &str = "ccc-package-ack-spool.json";
const MAX_ENTRIES: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AckSpoolEntry {
    pub queue_id: String,
    pub source_table: String,
    pub source_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_type: Option<String>,
    pub enqueued_unix_ms: i64,
    pub attempts: u32,
    pub last_error: String,
}

static SPOOL: Mutex<Option<Vec<AckSpoolEntry>>> = Mutex::new(None);

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

fn load_from_disk(app: &AppHandle) -> Result<Vec<AckSpoolEntry>, String> {
    let path = spool_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&raw).map_err(|e| format!("parse ack spool: {e}"))
}

fn persist(app: &AppHandle, entries: &[AckSpoolEntry]) -> Result<(), String> {
    let path = spool_path(app)?;
    let json = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

fn with_spool<F, R>(app: &AppHandle, f: F) -> Result<R, String>
where
    F: FnOnce(&mut Vec<AckSpoolEntry>) -> Result<R, String>,
{
    let mut guard = SPOOL.lock().map_err(|e| e.to_string())?;
    if guard.is_none() {
        *guard = Some(load_from_disk(app)?);
    }
    let vec = guard
        .as_mut()
        .ok_or_else(|| "ack spool unavailable".to_string())?;
    let out = f(vec)?;
    persist(app, vec)?;
    Ok(out)
}

pub fn pending_count(app: &AppHandle) -> u32 {
    with_spool(app, |v| Ok(v.len() as u32)).unwrap_or(0)
}

pub fn enqueue_batch(app: &AppHandle, entries: Vec<AckSpoolEntry>, last_error: &str) -> u32 {
    if entries.is_empty() {
        return pending_count(app);
    }
    with_spool(app, |v| {
        let now = now_unix_ms();
        for mut entry in entries {
            if let Some(existing) = v.iter_mut().find(|e| e.queue_id == entry.queue_id) {
                existing.attempts = existing.attempts.saturating_add(1);
                existing.last_error = last_error.to_string();
                if entry.written_path.is_some() {
                    existing.written_path = entry.written_path;
                }
                if entry.error_message.is_some() {
                    existing.error_message = entry.error_message;
                }
                continue;
            }
            entry.enqueued_unix_ms = now;
            entry.attempts = 1;
            entry.last_error = last_error.to_string();
            v.push(entry);
        }
        while v.len() > MAX_ENTRIES {
            v.remove(0);
        }
        Ok(v.len() as u32)
    })
    .unwrap_or(0)
}

pub fn claim_batch(app: &AppHandle, limit: usize) -> Vec<AckSpoolEntry> {
    with_spool(app, |v| {
        let n = limit.min(v.len());
        let batch: Vec<_> = v.drain(..n).collect();
        Ok(batch)
    })
    .unwrap_or_default()
}

pub fn remove_by_queue_ids(app: &AppHandle, queue_ids: &[String]) -> u32 {
    if queue_ids.is_empty() {
        return pending_count(app);
    }
    let set: std::collections::HashSet<&str> = queue_ids.iter().map(|s| s.as_str()).collect();
    with_spool(app, |v| {
        v.retain(|e| !set.contains(e.queue_id.as_str()));
        Ok(v.len() as u32)
    })
    .unwrap_or(0)
}
