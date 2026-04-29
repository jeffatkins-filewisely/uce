use crate::types::{RecentMemory, Rule};
use std::path::PathBuf;
use tauri::Manager;

fn memory_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir error: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create app data dir error: {e}"))?;
    Ok(dir.join("uce-memory.json"))
}

fn candidate_log_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir error: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create app data dir error: {e}"))?;
    Ok(dir.join("uce-candidates.log"))
}

fn custom_rules_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir error: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create app data dir error: {e}"))?;
    Ok(dir.join("uce-custom-known-rules.json"))
}

fn exclude_rules_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir error: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create app data dir error: {e}"))?;
    Ok(dir.join("uce-exclude-rules.json"))
}

pub fn load_memory(app: &tauri::AppHandle) -> Result<RecentMemory, String> {
    let path = memory_file(app)?;
    if !path.exists() {
        return Ok(RecentMemory::default());
    }
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

pub fn save_memory(app: &tauri::AppHandle, memory: &RecentMemory) -> Result<(), String> {
    let path = memory_file(app)?;
    let raw = serde_json::to_string(memory).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| e.to_string())
}

pub fn append_candidate_log(app: &tauri::AppHandle, line: &str) -> Result<(), String> {
    use std::io::Write;
    let path = candidate_log_file(app)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{line}").map_err(|e| e.to_string())
}

/// Creates `uce-candidates.log` with a bootstrap line if missing (candidate logging is otherwise lazy).
pub fn ensure_candidate_log_bootstrapped(app: &tauri::AppHandle) -> Result<(), String> {
    let path = candidate_log_file(app)?;
    if path.exists() {
        return Ok(());
    }
    append_candidate_log(app, r#"{"uce":"UCE_CANDIDATE_LOG_INITIALIZED"}"#)?;
    eprintln!(
        "UCE_CANDIDATE_LOG_INITIALIZED path={}",
        path.display()
    );
    Ok(())
}

pub fn load_custom_rules(app: &tauri::AppHandle) -> Result<Vec<Rule>, String> {
    let path = custom_rules_file(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

pub fn save_custom_rules(app: &tauri::AppHandle, rules: &[Rule]) -> Result<(), String> {
    let path = custom_rules_file(app)?;
    let raw = serde_json::to_string(rules).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| e.to_string())
}

pub fn load_exclude_rules(app: &tauri::AppHandle) -> Result<Vec<Rule>, String> {
    let path = exclude_rules_file(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

pub fn save_exclude_rules(app: &tauri::AppHandle, rules: &[Rule]) -> Result<(), String> {
    let path = exclude_rules_file(app)?;
    let raw = serde_json::to_string(rules).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| e.to_string())
}
