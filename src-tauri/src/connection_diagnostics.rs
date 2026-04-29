//! Persisted heartbeat outcomes + connection test + plain-text diagnostic report (secrets masked).

use arboard::Clipboard;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::sync::mpsc;
use std::time::Duration;
use tauri::AppHandle;
use tauri::Manager;

use crate::config::print_config;
use crate::pdf_watch_config;
use crate::services::capture_pipeline_status;
use crate::services::ccc_autodiscovery;
use crate::services::ccc_capture_diag;
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

fn url_overlay_classification(url: &str) -> &'static str {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("chrome-error:") || lower.contains("chromewebdata") {
        return "chrome_error_page";
    }
    if matches!(
        lower.trim(),
        "about:blank" | "about:srcdoc"
    ) || lower.starts_with("about:blank?")
    {
        return "about_blank";
    }
    if uce_webview_url::url_looks_like_loaded_app_ui(url) {
        return "app_ui";
    }
    "other"
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
                    let classification = url_overlay_classification(s);
                    json!({
                        "loaded": loaded,
                        "url": s,
                        "classification": classification,
                    })
                }
                Err(e) => json!({ "loaded": false, "url": format!("url() error: {e}"), "classification": "error" }),
            },
            None => json!({ "loaded": false, "url": "<no main window>", "classification": "no_window" }),
        };
        let _ = tx.send(v);
    }) {
        return json!({ "loaded": false, "url": format!("run_on_main_thread: {e}"), "classification": "error" });
    }
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(v) => v,
        Err(e) => json!({ "loaded": false, "url": format!("overlay snapshot timeout: {e}"), "classification": "error" }),
    }
}

fn capture_pipeline_snapshot(app: &AppHandle) -> serde_json::Value {
    let pw = pdf_watch_config::load_pdf_watch_config(app);
    let roots = pdf_watch_config::office_intercept_watch_roots(app);
    let paired: Vec<serde_json::Value> = roots
        .iter()
        .take(48)
        .map(|(p, rule)| {
            json!({
                "path": p.to_string_lossy(),
                "rule": rule,
            })
        })
        .collect();

    let pdf_watch_path = app
        .path()
        .app_data_dir()
        .map(|d| d.join("uce-pdf-watch.json").to_string_lossy().into_owned())
        .unwrap_or_else(|_| "(unknown)".to_string());

    let ccc_seen = ccc_capture_diag::last_ccc_files_seen();
    let last_20_ccc: Vec<String> = ccc_seen
        .iter()
        .rev()
        .take(20)
        .rev()
        .cloned()
        .collect();
    let ccc_temp = print_config::ccc_temp_watch_path();

    let printer_alert = serde_json::to_value(crate::services::printer_alert_policy::policy_snapshot())
        .unwrap_or(json!({}));

    let mut cap = json!({
        "pdf_watch_json_path": pdf_watch_path,
        "capture_pipeline_status": capture_pipeline_status::status_label(),
        "capture_pipeline_failure_detail": capture_pipeline_status::failure_reason(),
        "watcher_platform": if cfg!(windows) { "windows" } else { "unsupported" },
        "ccc_temp_watch_path": ccc_temp.to_string_lossy(),
        "watched_incoming_root": print_config::watched_incoming_root().to_string_lossy(),
        "ccc_temp_watch_only_env": print_config::ccc_temp_watch_only(),
        "printer_alert": printer_alert,
        "auto_discovered_ccc_dirs": pw.auto_discovered_ccc_dirs,
        "general_document_capture_enabled": pw.general_document_capture_enabled,
        "general_min_file_bytes": pw.general_min_file_bytes,
        "min_pdf_bytes_config": pw.min_pdf_bytes,
        "extra_dirs_count": pw.extra_dirs.len(),
        "office_intercept_extra_dirs_count": pw.office_intercept_extra_dirs.len(),
        "watch_root_count": roots.len(),
        "watch_roots": paired,
        "last_files_seen_ring_buffer": ccc_seen,
        "last_20_ccc_files_seen": last_20_ccc,
        "trace_hints": [
            "Connection OK but no captures: confirm PDFs land under a watched folder — CCC Temp\\\\CCC, FileWisely Incoming, or (if enabled) Documents/Downloads.",
            "CCC temp: OS watcher can miss fast writes — 1s polling backup logs UCE_POLL_SCAN_* / UCE_POLL_DETECTED_FILE.",
            "Trace capture: stderr UCE_FILE_DETECTED_RAW, UCE_PIPELINE_CONTEXT, UCE_FILE_ACCEPTED, UCE_FILE_COPY_*, UCE_RUST_EMIT_INCOMING, UCE_FILE_REJECTED — run UCE from cmd.",
            "Pipeline rings (this report): last_detected_files, last_rejected_files, last_accepted_files, last_copy_*, last_emitted_incoming_files, last_upload_* — see JSON keys on capture_pipeline.",
            "When UCE_CCC_TEMP_WATCH_ONLY is false, CCC Temp\\\\CCC PDFs still use the same handle_path → emit_uce_incoming_pdf path as other watch roots (not the CCC batch queue).",
            "Popups: stderr UCE_UI_NATIVE_ALERT kind=printer_severe|printer_repair_uac|webview_load_failed|dev_server_unreachable before each Windows MessageBox. Env UCE_SUPPRESS_PRINTER_NATIVE_ALERT=1 or localStorage uce_suppress_printer_severe_modal=1.",
            "JS console: UCE_UPLOAD_STARTED / UCE_UPLOAD_SUCCESS|SKIPPED|FAILED, UCE_UI_CLIENT kind=printer_severe_modal — DevTools when WebView is up.",
            "Search stderr for UCE_GENERAL_FILE_* / UCE_CCC_* — run UCE from Command Prompt to see lines.",
            "Printer: default warning-only — no MessageBox unless UCE_PRINTER_REQUIRED=1 or localStorage uce_printer_required=1 (sync via uce_sync_printer_ui_policy). CCC temp + watcher running forces warning_only. See capture_pipeline.printer_alert and stderr UCE_PRINTER_WARNING_ONLY.",
            "Toast every ~25s: health attention — expand Connection Doctor status (capture_pipeline) for printer/upload stale.",
            "WebView 'Could not load': classification chrome_error_page in diagnostics — start Vite (dev) or reinstall (prod)."
        ],
    });

    if let serde_json::Value::Object(ref mut m) = cap {
        if let serde_json::Value::Object(pt) = crate::services::pipeline_stage_diag::snapshot_json() {
            for (k, v) in pt {
                m.insert(k, v);
            }
        }
    }

    if let (
        serde_json::Value::Object(ref mut m),
        serde_json::Value::Object(extra),
    ) = (&mut cap, ccc_autodiscovery::diagnostics_snapshot())
    {
        for (k, v) in extra {
            m.insert(k, v);
        }
    }

    cap
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
        "capture_pipeline": capture_pipeline_snapshot(&app),
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

fn format_capture_pipeline_plain(cp: &serde_json::Value) -> String {
    let gstr = |k: &str| -> String {
        cp.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string()
    };
    let gbool = |k: &str| -> String {
        cp.get(k)
            .and_then(|x| x.as_bool())
            .map(|b| b.to_string())
            .unwrap_or_else(|| "(missing)".to_string())
    };
    let detail = cp
        .get("capture_pipeline_failure_detail")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("(none)");
    let mut out = String::new();
    out.push_str("capture_pipeline_status: ");
    out.push_str(&gstr("capture_pipeline_status"));
    out.push('\n');
    out.push_str("capture_pipeline_failure_detail: ");
    out.push_str(detail);
    out.push('\n');
    out.push_str("watcher_platform: ");
    out.push_str(&gstr("watcher_platform"));
    out.push('\n');
    out.push_str("pdf_watch_json_path: ");
    out.push_str(&gstr("pdf_watch_json_path"));
    out.push('\n');
    out.push_str("ccc_temp_watch_path: ");
    out.push_str(&gstr("ccc_temp_watch_path"));
    out.push('\n');
    out.push_str("watched_incoming_root (FW vs CCC temp): ");
    out.push_str(&gstr("watched_incoming_root"));
    out.push('\n');
    out.push_str("ccc_temp_watch_only_env (UCE_CCC_TEMP_WATCH_ONLY): ");
    out.push_str(&gbool("ccc_temp_watch_only_env"));
    out.push('\n');
    if let Some(pa) = cp.get("printer_alert") {
        out.push_str("printer_alert (policy snapshot):\n");
        out.push_str(&serde_json::to_string_pretty(pa).unwrap_or_else(|_| "{}".into()));
        out.push('\n');
    }
    out.push_str("--- pipeline stage rings (recent, Connection Doctor JSON keys) ---\n");
    for key in [
        "last_detected_files",
        "last_rejected_files",
        "last_accepted_files",
        "last_copy_attempts",
        "last_copy_successes",
        "last_copy_failures",
        "last_emitted_incoming_files",
        "last_upload_attempts",
        "last_upload_successes",
        "last_upload_failures",
    ] {
        if let Some(v) = cp.get(key) {
            out.push_str(key);
            out.push_str(":\n");
            out.push_str(&serde_json::to_string_pretty(v).unwrap_or_else(|_| "[]".into()));
            out.push_str("\n\n");
        }
    }
    out.push_str("general_document_capture_enabled: ");
    out.push_str(&gbool("general_document_capture_enabled"));
    out.push('\n');
    out.push_str("auto_discovered_ccc_dirs (persisted in uce-pdf-watch.json):\n");
    if let Some(arr) = cp.get("auto_discovered_ccc_dirs").and_then(|x| x.as_array()) {
        if arr.is_empty() {
            out.push_str("  (none)\n");
        }
        for x in arr {
            if let Some(s) = x.as_str() {
                out.push_str(&format!("  - {}\n", s));
            }
        }
    } else {
        out.push_str("  (missing)\n");
    }
    if let Some(ms) = cp
        .get("ccc_autodiscovery_last_run_unix_ms")
        .and_then(|x| x.as_i64())
    {
        out.push_str(&format!("ccc_autodiscovery_last_run_unix_ms: {}\n", ms));
    } else {
        out.push_str("ccc_autodiscovery_last_run_unix_ms: (never)\n");
    }
    out.push_str("ccc_autodiscovery_confidence: ");
    out.push_str(&format!(
        "{:.3}\n",
        cp.get("ccc_autodiscovery_confidence")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0)
    ));
    out.push_str("ccc_autodiscovery_candidates (sample):\n");
    if let Some(arr) = cp
        .get("ccc_autodiscovery_candidates")
        .and_then(|x| x.as_array())
    {
        if arr.is_empty() {
            out.push_str("  (none)\n");
        }
        for x in arr.iter().take(24) {
            if let Some(s) = x.as_str() {
                out.push_str(&format!("  - {}\n", s));
            }
        }
        if arr.len() > 24 {
            out.push_str(&format!("  … {} more\n", arr.len() - 24));
        }
    } else {
        out.push_str("  (missing)\n");
    }
    out.push_str("watch_root_count: ");
    if let Some(n) = cp.get("watch_root_count").and_then(|x| x.as_u64()) {
        out.push_str(&n.to_string());
    } else {
        out.push_str("(missing)");
    }
    out.push('\n');

    out.push_str("watch_roots:\n");
    if let Some(arr) = cp.get("watch_roots").and_then(|x| x.as_array()) {
        if arr.is_empty() {
            out.push_str("  (none listed)\n");
        }
        for (i, item) in arr.iter().enumerate().take(48) {
            let path = item
                .get("path")
                .and_then(|x| x.as_str())
                .unwrap_or("?");
            let rule = item
                .get("rule")
                .and_then(|x| x.as_str())
                .unwrap_or("?");
            out.push_str(&format!("  [{}] {}  (rule={})\n", i + 1, path, rule));
        }
        if arr.len() > 48 {
            out.push_str(&format!(
                "  … {} more roots omitted (see full capture_pipeline JSON)\n",
                arr.len() - 48
            ));
        }
    } else {
        out.push_str("  (missing)\n");
    }

    out.push_str("last_files_seen_ring_buffer (CCC-related, chronological):\n");
    if let Some(arr) = cp.get("last_files_seen_ring_buffer").and_then(|x| x.as_array()) {
        if arr.is_empty() {
            out.push_str("  (empty — no CCC-related files recorded yet this session)\n");
        }
        for x in arr {
            if let Some(s) = x.as_str() {
                out.push_str(&format!("  - {}\n", s));
            }
        }
    } else {
        out.push_str("  (missing)\n");
    }

    out.push_str("last_20_ccc_files_seen:\n");
    if let Some(arr) = cp.get("last_20_ccc_files_seen").and_then(|x| x.as_array()) {
        if arr.is_empty() {
            out.push_str("  (empty)\n");
        }
        for x in arr {
            if let Some(s) = x.as_str() {
                out.push_str(&format!("  - {}\n", s));
            }
        }
    } else {
        out.push_str("  (missing)\n");
    }

    out.push_str("trace_hints:\n");
    if let Some(arr) = cp.get("trace_hints").and_then(|x| x.as_array()) {
        for x in arr {
            if let Some(s) = x.as_str() {
                out.push_str(&format!("  - {}\n", s));
            }
        }
    } else {
        out.push_str("  (missing)\n");
    }

    out.push_str("\n--- capture_pipeline (full JSON) ---\n");
    out.push_str(
        &serde_json::to_string_pretty(cp).unwrap_or_else(|_| "{}".to_string()),
    );
    out.push('\n');
    out
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
    let main_class = v
        .get("main_overlay_loaded")
        .and_then(|m| m.get("classification"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let cp = v.get("capture_pipeline").cloned().unwrap_or(json!({}));
    let capture_plain = format_capture_pipeline_plain(&cp);

    let lines = format!(
        "UCE Connection + Capture Diagnostic Report\n\
\n\
=== Connection health ===\n\
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
Main URL classification: {}\n\
\n\
=== Capture pipeline health (watcher + CCC) ===\n\
{}\n\
=== Recent connection log (tail) ===\n\
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
        main_class,
        capture_plain,
        recent
    );

    Clipboard::new()
        .and_then(|mut c| c.set_text(lines.clone()))
        .map_err(|e| format!("clipboard: {e}"))?;
    Ok(lines)
}
