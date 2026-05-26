//! CCC Import folder (`settings.json` next to tenant config) + first-run / tray folder picker.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::AppHandle;
use tauri::Manager;

pub const DEFAULT_CCC_PACKAGE_ROOT: &str = r"C:\FileWisely\CCC Import";

const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SidekickSettings {
    #[serde(default)]
    pub ccc_package_root: String,
    #[serde(default)]
    pub first_run_completed: bool,
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("create app data dir: {e}"))?;
    Ok(dir.join(SETTINGS_FILE))
}

pub fn load_settings(app: &AppHandle) -> SidekickSettings {
    let path = match settings_path(app) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[UCE] settings path: {e}");
            return SidekickSettings::default();
        }
    };
    if !path.exists() {
        return SidekickSettings::default();
    }
    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            eprintln!("[UCE] settings.json parse error: {e}");
            SidekickSettings::default()
        }),
        Err(e) => {
            eprintln!("[UCE] settings.json read error: {e}");
            SidekickSettings::default()
        }
    }
}

fn write_settings(app: &AppHandle, settings: &SidekickSettings) -> Result<(), String> {
    let path = settings_path(app)?;
    let raw = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(path, raw).map_err(|e| e.to_string())
}

pub fn effective_ccc_package_root(settings: &SidekickSettings) -> Option<String> {
    let root = settings.ccc_package_root.trim();
    if root.is_empty() {
        None
    } else {
        Some(normalize_root_path(root))
    }
}

fn normalize_root_path(path: &str) -> String {
    Path::new(path.trim())
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_string()
}

pub fn set_ccc_package_root(app: &AppHandle, root: &str) -> Result<String, String> {
    let normalized = normalize_root_path(root);
    if normalized.is_empty() {
        return Err("CCC Import folder path is empty".to_string());
    }
    fs::create_dir_all(&normalized).map_err(|e| format!("cannot create folder: {e}"))?;
    let mut settings = load_settings(app);
    settings.ccc_package_root = normalized.clone();
    settings.first_run_completed = true;
    write_settings(app, &settings)?;
    eprintln!(
        "CCC_IMPORT_ROOT_SET path={}",
        settings.ccc_package_root
    );
    Ok(normalized)
}

/// First launch: prompt for CCC Import root (Windows folder dialog); default on cancel.
pub fn ensure_first_run_configured(app: &AppHandle) {
    let settings = load_settings(app);
    if settings.first_run_completed && effective_ccc_package_root(&settings).is_some() {
        return;
    }

    let initial = if settings.ccc_package_root.trim().is_empty() {
        DEFAULT_CCC_PACKAGE_ROOT.to_string()
    } else {
        settings.ccc_package_root.clone()
    };

    let chosen = pick_folder_dialog(
        "Where should FileWisely save CCC Import folders?",
        &initial,
    )
    .unwrap_or_else(|| initial);

    if let Err(e) = set_ccc_package_root(app, &chosen) {
        eprintln!("[UCE] first-run CCC Import folder: {e}");
    }
}

#[cfg(windows)]
pub fn pick_folder_dialog(description: &str, initial_path: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let desc_escaped = description.replace('\'', "''");
    let init_escaped = initial_path.replace('\'', "''");
    let script = format!(
        r#"Add-Type -AssemblyName System.Windows.Forms
$d = New-Object System.Windows.Forms.FolderBrowserDialog
$d.Description = '{desc_escaped}'
$d.SelectedPath = '{init_escaped}'
$d.ShowNewFolderButton = $true
if ($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{ Write-Output $d.SelectedPath }}"#
    );

    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

#[cfg(not(windows))]
pub fn pick_folder_dialog(_description: &str, initial_path: &str) -> Option<String> {
    Some(initial_path.to_string())
}

pub fn open_folder_in_explorer(path: &str) -> Result<(), String> {
    let p = Path::new(path.trim());
    if !p.exists() {
        fs::create_dir_all(p).map_err(|e| format!("cannot create folder: {e}"))?;
    }
    open::that(p).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ccc_import_get_package_root(app: AppHandle) -> Result<String, String> {
    let settings = load_settings(&app);
    effective_ccc_package_root(&settings).ok_or_else(|| "ccc_package_root not configured".to_string())
}

#[tauri::command]
pub fn ccc_import_pick_and_set_root(app: AppHandle) -> Result<String, String> {
    let settings = load_settings(&app);
    let initial = effective_ccc_package_root(&settings)
        .unwrap_or_else(|| DEFAULT_CCC_PACKAGE_ROOT.to_string());
    let chosen = pick_folder_dialog("Select CCC Import folder", &initial)
        .ok_or_else(|| "folder picker cancelled".to_string())?;
    set_ccc_package_root(&app, &chosen)
}

#[tauri::command]
pub fn ccc_import_open_root_folder(app: AppHandle) -> Result<(), String> {
    let root = ccc_import_get_package_root(app)?;
    open_folder_in_explorer(&root)
}
