//! Persisted heartbeat outcomes + connection test + plain-text diagnostic report (secrets masked).

use arboard::Clipboard;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::sync::mpsc;
use std::time::Duration;
use tauri::AppHandle;
use tauri::Manager;

use crate::tenant_config;
use crate::uce_webview_url;

const OUTCOME_FILE: &str = "uce-heartbeat-outcome.json";
const RECENT_LOG_MAX: usize = 40;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HeartbeatOutcomeRecord {
    pub last_unix_ms: i64,
    pub success: bool,
    pub category: String,
    #[serde(default)]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionTestResult {
    pub ok: bool,
    pub category: String,
    #[serde(default)]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub message: String,
}

fn outcome_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(OUTCOME_FILE))
}

fn read_outcome(app: &AppHandle) -> HeartbeatOutcomeRecord {
    let Ok(path) = outcome_path(app) else {
        return HeartbeatOutcomeRecord::default();
    };
    let Ok(bytes) = fs::read(&path) else {
        return HeartbeatOutcomeRecord::default();
    };
    serde_json::from_slice::<HeartbeatOutcomeRecord>(&bytes).unwrap_or_default()
}

fn write_outcome(app: &AppHandle, rec: &HeartbeatOutcomeRecord) -> Result<(), String> {
    let path = outcome_path(app)?;
    let raw = serde_json::to_string_pretty(rec).map_err(|e| e.to_string())?;
    fs::write(path, raw).map_err(|e| e.to_string())
}

fn push_recent_log(app: &AppHandle, line: &str) {
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("uce-connection-recent.log");
    let mut lines: Vec<String> = fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect();
    let stamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    lines.push(format!("{stamp} {line}"));
    while lines.len() > RECENT_LOG_MAX {
        lines.remove(0);
    }
    let _ = fs::write(path, lines.join("\n"));
}

pub fn mask_secret(s: &str) -> String {
    let t = s.trim();
    if t.len() <= 10 {
        return if t.is_empty() {
            "(empty)".to_string()
        } else {
            "***".to_string()
        };
    }
    format!(
        "{}…{}",
        &t[..6.min(t.len())],
        &t[t.len().saturating_sub(4)..]
    )
}

#[derive(Debug, Deserialize)]
pub struct RecordOutcomePayload {
    pub success: bool,
    pub category: String,
    pub http_status: Option<u16>,
    pub message: Option<String>,
}

#[tauri::command]
pub fn uce_record_heartbeat_outcome(
    app: AppHandle,
    payload: RecordOutcomePayload,
) -> Result<(), String> {
    let msg = payload.message.unwrap_or_default();
    let line = format!(
        "outcome success={} category={} http={:?} msg={}",
        payload.success,
        payload.category,
        payload.http_status,
        msg.chars().take(200).collect::<String>()
    );
    push_recent_log(&app, &line);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let rec = HeartbeatOutcomeRecord {
        last_unix_ms: now,
        success: payload.success,
        category: payload.category.clone(),
        http_status: payload.http_status,
        message: msg.chars().take(2000).collect(),
    };
    write_outcome(&app, &rec)?;
    Ok(())
}

fn main_overlay_snapshot(app: &AppHandle) -> serde_json::Value {
    let (tx, rx) = mpsc::channel::<serde_json::Value>();
    let app_c = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        let v = match app_c.get_webview_window("main") {
            Some(w) => match w.url() {
                Ok(u) => {
                    let s = u.as_str();
                    let loaded = uce_webview_url::url_looks_like_loaded_app_ui(s);
                    json!({ "loaded": loaded, "url": s })
                }
                Err(e) => json!({ "loaded": false, "url": format!("url() error: {e}") }),
            },
            None => json!({ "loaded": false, "url": "<no main window>" }),
        };
        let _ = tx.send(v);
    }) {
        return json!({ "loaded": false, "url": format!("run_on_main_thread: {e}") });
    }
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(v) => v,
        Err(e) => json!({ "loaded": false, "url": format!("overlay snapshot timeout: {e}") }),
    }
}

#[tauri::command]
pub fn uce_get_connection_diagnostics(app: AppHandle) -> Result<serde_json::Value, String> {
    let cfg = tenant_config::load_tenant_config(&app)?;
    let outcome = read_outcome(&app);
    let tenant_path = tenant_config::tenant_config_path_for_display(&app)?;

    let bid = cfg.business_id.trim();
    let backend = cfg.backend_url.trim();
    let anon = cfg.anon_key.trim();

    let mut missing: Vec<String> = Vec::new();
    if bid.is_empty() {
        missing.push("MISSING_BUSINESS_ID".to_string());
    }
    if backend.is_empty() {
        missing.push("MISSING_BACKEND_URL".to_string());
    }
    if anon.is_empty() {
        missing.push("MISSING_AUTH_KEY".to_string());
    }
    let missing_config = !missing.is_empty();

    let host = if backend.is_empty() {
        None
    } else {
        url::Url::parse(backend)
            .ok()
            .and_then(|u| u.host().map(|h| h.to_string()))
    };

    let main_overlay = main_overlay_snapshot(&app);

    Ok(json!({
        "uce_version": env!("CARGO_PKG_VERSION"),
        "machine_name": sysinfo::System::host_name().unwrap_or_else(|| "unknown".to_string()),
        "os_info": sysinfo::System::long_os_version().unwrap_or_else(|| "unknown".to_string()),
        "app_mode": if cfg!(debug_assertions) { "dev" } else { "prod" },
        "config_path": tenant_path,
        "business_id_present": !bid.is_empty(),
        "backend_url_present": !backend.is_empty(),
        "anon_key_present": !anon.is_empty(),
        "missing_config_categories": missing,
        "missing_config": missing_config,
        "ingest_host": host,
        "anon_key_masked": if anon.is_empty() {
            String::new()
        } else {
            mask_secret(anon)
        },
        "last_heartbeat_unix_ms": outcome.last_unix_ms,
        "last_heartbeat_success": outcome.success,
        "last_heartbeat_category": outcome.category,
        "last_heartbeat_http_status": outcome.http_status,
        "last_heartbeat_message": outcome.message,
        "main_overlay_loaded": main_overlay,
    }))
}

#[tauri::command]
pub async fn uce_test_ingest_connection(
    app: AppHandle,
    device_id: Option<String>,
) -> Result<ConnectionTestResult, String> {
    eprintln!("UCE_CONNECTION_TEST_STARTED");
    let cfg = tenant_config::load_tenant_config(&app)?;

    let bid = cfg.business_id.trim();
    let upload = cfg.backend_url.trim();
    let key = cfg.anon_key.trim();

    if bid.is_empty() {
        let r = fail_test("MISSING_BUSINESS_ID", None, "business_id is empty");
        finish_test(&app, &r);
        eprintln!("UCE_CONNECTION_TEST_FAILED category=MISSING_BUSINESS_ID");
        return Ok(r);
    }
    if upload.is_empty() {
        let r = fail_test("MISSING_BACKEND_URL", None, "backend_url / ingest URL is empty");
        finish_test(&app, &r);
        eprintln!("UCE_CONNECTION_TEST_FAILED category=MISSING_BACKEND_URL");
        return Ok(r);
    }
    if key.is_empty() {
        let r = fail_test("MISSING_AUTH_KEY", None, "anon_key is empty");
        finish_test(&app, &r);
        eprintln!("UCE_CONNECTION_TEST_FAILED category=MISSING_AUTH_KEY");
        return Ok(r);
    }

    let version = env!("CARGO_PKG_VERSION").to_string();
    let device_name = sysinfo::System::host_name().unwrap_or_else(|| "unknown".to_string());
    let os_info = sysinfo::System::long_os_version().unwrap_or_else(|| "unknown".to_string());
    let device_id = device_id.unwrap_or_else(|| format!("uce-test-{}", std::process::id()));

    let body = json!({
        "action": "heartbeat",
        "business_id": bid,
        "device_id": device_id,
        "device_name": device_name,
        "agent_version": version,
        "os_info": os_info,
        "user_id": "",
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = match client
        .post(upload)
        .header("Authorization", format!("Bearer {key}"))
        .header("apikey", key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let msg = e.to_string();
            let r = fail_test("NETWORK_ERROR", None, &msg);
            finish_test(&app, &r);
            eprintln!("UCE_CONNECTION_TEST_FAILED category=NETWORK_ERROR msg={msg}");
            return Ok(r);
        }
    };

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    let cat = categorize_http_status(status);

    if (200..300).contains(&status) {
        eprintln!("UCE_CONNECTION_TEST_SUCCESS http={status}");
        let rec = HeartbeatOutcomeRecord {
            last_unix_ms: now_ms(),
            success: true,
            category: "HEARTBEAT_OK".to_string(),
            http_status: Some(status),
            message: text.chars().take(500).collect(),
        };
        let _ = write_outcome(&app, &rec);
        push_recent_log(
            &app,
            &format!("CONNECTION_TEST_OK http={status} body_len={}", text.len()),
        );
        Ok(ConnectionTestResult {
            ok: true,
            category: "HEARTBEAT_OK".to_string(),
            http_status: Some(status),
            message: "OK".to_string(),
        })
    } else {
        eprintln!(
            "UCE_CONNECTION_TEST_FAILED category={} http={} body={}",
            cat,
            status,
            text.chars().take(200).collect::<String>()
        );
        let r = fail_test(cat, Some(status), &text);
        finish_test(&app, &r);
        Ok(r)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn categorize_http_status(status: u16) -> &'static str {
    match status {
        401 => "HTTP_401",
        403 => "HTTP_403",
        404 => "HTTP_404",
        500..=599 => "HTTP_500",
        _ => "UNKNOWN",
    }
}

fn fail_test(
    category: &str,
    http_status: Option<u16>,
    message: &str,
) -> ConnectionTestResult {
    ConnectionTestResult {
        ok: false,
        category: category.to_string(),
        http_status,
        message: message.chars().take(2000).collect(),
    }
}

fn finish_test(app: &AppHandle, r: &ConnectionTestResult) {
    let rec = HeartbeatOutcomeRecord {
        last_unix_ms: now_ms(),
        success: r.ok,
        category: r.category.clone(),
        http_status: r.http_status,
        message: r.message.clone(),
    };
    let _ = write_outcome(app, &rec);
}

#[tauri::command]
pub fn uce_copy_diagnostic_report(app: AppHandle) -> Result<String, String> {
    let v = uce_get_connection_diagnostics(app.clone())?;
    let recent_path = app
        .path()
        .app_data_dir()
        .map(|d| d.join("uce-connection-recent.log"))
        .map_err(|e| e.to_string())?;
    let recent = fs::read_to_string(&recent_path).unwrap_or_default();

    let main_loaded = v
        .get("main_overlay_loaded")
        .and_then(|m| m.get("loaded"))
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let main_url = v
        .get("main_overlay_loaded")
        .and_then(|m| m.get("url"))
        .and_then(|x| x.as_str())
        .unwrap_or("");

    let lines = format!(
        "UCE Connection Diagnostic Report\n\
UCE version: {}\n\
Machine: {}\n\
OS: {}\n\
App mode: {}\n\
Config file: {}\n\
Business ID: {}\n\
Backend / ingest URL: {}\n\
Anon key (masked): {}\n\
Missing config flags: {:?}\n\
Last heartbeat (unix ms): {}\n\
Last heartbeat success: {}\n\
Last heartbeat category: {}\n\
Last HTTP status: {:?}\n\
Last error message: {}\n\
Main overlay loaded: {}\n\
Main URL snapshot: {}\n\
\n\
--- Recent connection log (tail) ---\n\
{}\n",
        v["uce_version"].as_str().unwrap_or(""),
        v["machine_name"].as_str().unwrap_or(""),
        v["os_info"].as_str().unwrap_or(""),
        v["app_mode"].as_str().unwrap_or(""),
        v["config_path"].as_str().unwrap_or(""),
        if v["business_id_present"].as_bool() == Some(true) {
            "set"
        } else {
            "missing"
        },
        if v["backend_url_present"].as_bool() == Some(true) {
            "set"
        } else {
            "missing"
        },
        v["anon_key_masked"].as_str().unwrap_or("(n/a)"),
        v["missing_config_categories"],
        v["last_heartbeat_unix_ms"],
        v["last_heartbeat_success"],
        v["last_heartbeat_category"].as_str().unwrap_or(""),
        v["last_heartbeat_http_status"],
        v["last_heartbeat_message"].as_str().unwrap_or(""),
        main_loaded,
        main_url,
        recent
    );

    Clipboard::new()
        .and_then(|mut c| c.set_text(lines.clone()))
        .map_err(|e| format!("clipboard: {e}"))?;
    Ok(lines)
}
