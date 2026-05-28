//! Claim → download → write → ack loop for FileWisely CCC package queue (15s when online).

use crate::ccc_import_settings::{self, effective_ccc_package_root};
use crate::device_id;
use crate::tenant_config;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::menu::MenuItem;
use tauri::{AppHandle, Emitter, Wry};

const POLL_INTERVAL_SECS: u64 = 15;
const CLAIM_LIMIT: u32 = 25;
const HTTP_TIMEOUT_SECS: u64 = 120;

static SYNCING_COUNT: AtomicU32 = AtomicU32::new(0);
static SYNC_PAUSED: AtomicBool = AtomicBool::new(false);
static OFFLINE: Mutex<bool> = Mutex::new(false);
static LAST_CLAIM_ERROR: Mutex<String> = Mutex::new(String::new());
static LAST_WRITE_ERROR: Mutex<String> = Mutex::new(String::new());
static LAST_WRITE_OK_MS: AtomicI64 = AtomicI64::new(0);
static LAST_CLAIM_BATCH_COUNT: AtomicU32 = AtomicU32::new(0);
static LAST_CLAIM_PARSED_MS: AtomicI64 = AtomicI64::new(0);
static LAST_ACK_OK_MS: AtomicI64 = AtomicI64::new(0);
static LAST_ACK_OK_COUNT: AtomicU32 = AtomicU32::new(0);
static LAST_ACK_FAIL_COUNT: AtomicU32 = AtomicU32::new(0);
static LAST_ACK_ERROR: Mutex<String> = Mutex::new(String::new());
static IMPORT_WRITABLE: AtomicBool = AtomicBool::new(true);
static CCC_SYNC_STATUS_ITEM: OnceLock<MenuItem<Wry>> = OnceLock::new();
static PAUSE_SYNC_ITEM: OnceLock<MenuItem<Wry>> = OnceLock::new();
static RESUME_SYNC_ITEM: OnceLock<MenuItem<Wry>> = OnceLock::new();

/// Clone stored when the tray menu is built (`main.rs`).
pub fn register_ccc_sync_status_item(item: MenuItem<Wry>) {
    let _ = CCC_SYNC_STATUS_ITEM.set(item);
}

pub fn register_pause_resume_items(pause: MenuItem<Wry>, resume: MenuItem<Wry>) {
    let _ = PAUSE_SYNC_ITEM.set(pause);
    let _ = RESUME_SYNC_ITEM.set(resume);
    refresh_pause_resume_menu();
}

pub fn is_sync_paused() -> bool {
    SYNC_PAUSED.load(Ordering::Relaxed)
}

pub fn is_ccc_offline() -> bool {
    *OFFLINE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn syncing_count() -> u32 {
    SYNCING_COUNT.load(Ordering::Relaxed)
}

pub fn set_sync_paused(app: &AppHandle, paused: bool) {
    SYNC_PAUSED.store(paused, Ordering::Relaxed);
    refresh_pause_resume_menu();
    crate::device_health::refresh_tray(app);
    if paused {
        let _ = app.emit("uce:pause", ());
        eprintln!("CCC_PACKAGE_SYNC_PAUSED");
    } else {
        let _ = app.emit("uce:resume", ());
        eprintln!("CCC_PACKAGE_SYNC_RESUMED");
    }
}

pub fn refresh_pause_resume_menu() {
    let paused = is_sync_paused();
    if let Some(item) = PAUSE_SYNC_ITEM.get() {
        let _ = item.set_enabled(!paused);
    }
    if let Some(item) = RESUME_SYNC_ITEM.get() {
        let _ = item.set_enabled(paused);
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ClaimItem {
    queue_id: String,
    /// `mirror_file` (default), `delete_folder`, `delete_file`, `archive_folder`, …
    #[serde(default)]
    action_type: Option<String>,
    /// Edge: `ro_folder` on mirror_file items.
    #[serde(alias = "ro_folder", default)]
    ro_folder_name: Option<String>,
    /// e.g. `photos/check_in`, `estimates`, `payments` — preferred layout when set.
    #[serde(default)]
    sub_folder: Option<String>,
    /// Legacy photo bucket when `sub_folder` is absent.
    #[serde(default)]
    bucket: Option<String>,
    #[serde(default)]
    signed_url: Option<String>,
    /// Edge: `filename_hint` on mirror_file items.
    #[serde(alias = "filename_hint", default)]
    filename: Option<String>,
    /// RO folder name (or relative path under CCC Import root) for cleanup jobs.
    #[serde(default)]
    target_path_hint: Option<String>,
    source_table: String,
    source_id: String,
}

impl ClaimItem {
    fn action(&self) -> &str {
        self.action_type
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("mirror_file")
    }
}

#[derive(Serialize)]
struct ClaimBatchBody<'a> {
    business_id: &'a str,
    device_id: &'a str,
    limit: u32,
}

/// One row in `ccc-package-ack` POST body `items[]`.
#[derive(Clone)]
struct PendingAck {
    queue_id: String,
    source_table: String,
    source_id: String,
    status: String,
    error_message: Option<String>,
    written_path: Option<String>,
    sub_folder: Option<String>,
    action_type: Option<String>,
}

#[derive(Serialize)]
struct AckItemBody<'a> {
    queue_id: &'a str,
    source_table: &'a str,
    source_id: &'a str,
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    written_path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_message: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub_folder: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_type: Option<&'a str>,
}

#[derive(Serialize)]
struct AckBatchBody<'a> {
    business_id: &'a str,
    device_id: &'a str,
    items: Vec<AckItemBody<'a>>,
}

pub fn tray_ccc_sync_label() -> String {
    if is_sync_paused() {
        return "CCC sync: Paused".to_string();
    }
    if *OFFLINE.lock().unwrap_or_else(|e| e.into_inner()) {
        return "CCC sync: Offline".to_string();
    }
    let n = SYNCING_COUNT.load(Ordering::Relaxed);
    if n > 0 {
        format!("CCC sync: {n} syncing…")
    } else {
        "CCC sync: 0 pending".to_string()
    }
}

pub fn refresh_tray_ccc_sync_item(_app: &AppHandle) {
    let label = tray_ccc_sync_label();
    if let Some(item) = CCC_SYNC_STATUS_ITEM.get() {
        let _ = item.set_text(label);
    }
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn cap_err(s: &str) -> String {
    s.chars().take(500).collect()
}

pub fn set_ccc_import_writable(writable: bool) {
    IMPORT_WRITABLE.store(writable, Ordering::Relaxed);
}

pub fn ccc_import_writable() -> bool {
    IMPORT_WRITABLE.load(Ordering::Relaxed)
}

pub fn last_ccc_claim_error() -> String {
    LAST_CLAIM_ERROR
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

pub fn last_ccc_write_error() -> String {
    LAST_WRITE_ERROR
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

pub fn last_ccc_write_unix_ms() -> i64 {
    LAST_WRITE_OK_MS.load(Ordering::Relaxed)
}

pub fn last_ccc_claim_batch_count() -> u32 {
    LAST_CLAIM_BATCH_COUNT.load(Ordering::Relaxed)
}

pub fn last_ccc_claim_parsed_unix_ms() -> i64 {
    LAST_CLAIM_PARSED_MS.load(Ordering::Relaxed)
}

pub fn last_ccc_ack_ok_unix_ms() -> i64 {
    LAST_ACK_OK_MS.load(Ordering::Relaxed)
}

pub fn last_ccc_ack_ok_count() -> u32 {
    LAST_ACK_OK_COUNT.load(Ordering::Relaxed)
}

pub fn last_ccc_ack_fail_count() -> u32 {
    LAST_ACK_FAIL_COUNT.load(Ordering::Relaxed)
}

pub fn last_ccc_ack_error() -> String {
    LAST_ACK_ERROR
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

fn note_claim_error(msg: &str) {
    let m = cap_err(msg);
    if let Ok(mut g) = LAST_CLAIM_ERROR.lock() {
        *g = m.clone();
    }
    crate::device_health::set_last_error(format!("CCC claim: {m}"));
}

fn clear_claim_error() {
    if let Ok(mut g) = LAST_CLAIM_ERROR.lock() {
        g.clear();
    }
}

fn note_write_error(msg: &str) {
    let m = cap_err(msg);
    if let Ok(mut g) = LAST_WRITE_ERROR.lock() {
        *g = m.clone();
    }
    crate::device_health::set_last_error(format!("CCC write: {m}"));
}

fn note_write_ok() {
    LAST_WRITE_OK_MS.store(now_unix_ms(), Ordering::Relaxed);
    if let Ok(mut g) = LAST_WRITE_ERROR.lock() {
        g.clear();
    }
    crate::device_health::note_ccc_sync_activity();
}

fn note_ack_error(msg: &str) {
    let m = cap_err(msg);
    if let Ok(mut g) = LAST_ACK_ERROR.lock() {
        *g = m.clone();
    }
    crate::device_health::set_last_error(format!("CCC ack: {m}"));
}

fn clear_ack_batch_counters() {
    LAST_ACK_OK_COUNT.store(0, Ordering::Relaxed);
    LAST_ACK_FAIL_COUNT.store(0, Ordering::Relaxed);
}

fn record_ack_ok() {
    LAST_ACK_OK_MS.store(now_unix_ms(), Ordering::Relaxed);
    LAST_ACK_OK_COUNT.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut g) = LAST_ACK_ERROR.lock() {
        g.clear();
    }
}

fn record_ack_fail(msg: &str) {
    LAST_ACK_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
    note_ack_error(msg);
}

fn set_offline(offline: bool, claim_reason: Option<&str>) {
    if let Ok(mut g) = OFFLINE.lock() {
        *g = offline;
    }
    if offline {
        if let Some(r) = claim_reason {
            note_claim_error(r);
        }
    } else {
        clear_claim_error();
    }
}

/// Edge may use snake_case or camelCase; jobs may be nested under `mirror_file` / `payload`.
fn parse_claim_batch_body(text: &str) -> Result<Vec<ClaimItem>, String> {
    use serde_json::Value;

    let root: Value =
        serde_json::from_str(text).map_err(|e| format!("claim JSON invalid: {e}"))?;

    let arr = if let Some(a) = root.as_array() {
        a
    } else if let Some(obj) = root.as_object() {
        let mut found: Option<&Vec<Value>> = None;
        for key in [
            "items",
            "jobs",
            "claimed",
            "claimed_items",
            "mirror_files",
        ] {
            if let Some(Value::Array(a)) = obj.get(key) {
                found = Some(a);
                break;
            }
        }
        found.ok_or_else(|| {
            let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
            format!("claim response missing items array (top-level keys: {keys:?})")
        })?
    } else {
        return Err("claim response root must be object or array".to_string());
    };

    let batch_defaults = batch_ack_defaults(&root);

    let mut items = Vec::with_capacity(arr.len());
    let mut skip_errors: Vec<String> = Vec::new();
    for (i, raw) in arr.iter().enumerate() {
        let norm = normalize_claim_item_value(raw.clone(), &batch_defaults);
        match serde_json::from_value::<ClaimItem>(norm) {
            Ok(item) => items.push(item),
            Err(e) => skip_errors.push(format!("[{i}]: {e}")),
        }
    }

    if items.is_empty() {
        if arr.is_empty() {
            return Ok(items);
        }
        let hint = if skip_errors.is_empty() {
            "no items deserialized".to_string()
        } else {
            format!("all {} items failed parse: {}", arr.len(), skip_errors.join("; "))
        };
        if text.contains("queue_id") || text.contains("queueId") || text.contains("signed_url")
            || text.contains("signedUrl")
        {
            return Err(format!(
                "{hint} — response contains job fields; likely shape mismatch"
            ));
        }
        return Err(hint);
    }

    if !skip_errors.is_empty() {
        eprintln!(
            "CCC_PACKAGE_CLAIM_PARTIAL skipped={} parsed={} errors={}",
            skip_errors.len(),
            items.len(),
            skip_errors.join("; ")
        );
    }

    Ok(items)
}

fn camel_to_snake_field(key: &str) -> &str {
    match key {
        "signedUrl" => "signed_url",
        "queueId" => "queue_id",
        "actionType" => "action_type",
        "roFolder" => "ro_folder",
        "roFolderName" => "ro_folder_name",
        "subFolder" => "sub_folder",
        "filenameHint" => "filename_hint",
        "targetPathHint" => "target_path_hint",
        "sourceTable" => "source_table",
        "sourceId" => "source_id",
        other => other,
    }
}

fn scalar_to_string(v: serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[derive(Default, Clone)]
struct BatchAckDefaults {
    source_table: Option<String>,
    source_id: Option<String>,
}

fn batch_ack_defaults(root: &serde_json::Value) -> BatchAckDefaults {
    let Some(obj) = root.as_object() else {
        return BatchAckDefaults::default();
    };
    BatchAckDefaults {
        source_table: obj
            .get("source_table")
            .or(obj.get("sourceTable"))
            .and_then(|v| scalar_to_string(v.clone())),
        source_id: obj
            .get("source_id")
            .or(obj.get("sourceId"))
            .and_then(|v| scalar_to_string(v.clone())),
    }
}

fn apply_ack_identity_defaults(out: &mut serde_json::Map<String, serde_json::Value>, batch: &BatchAckDefaults) {
    use serde_json::Value;

    // `source: { table, id }` (legacy / compact shapes)
    if let Some(Value::Object(src)) = out.remove("source") {
        if !out.contains_key("source_table") {
            if let Some(t) = src
                .get("table")
                .or(src.get("source_table"))
                .or(src.get("sourceTable"))
            {
                if let Some(s) = scalar_to_string(t.clone()) {
                    out.insert("source_table".to_string(), Value::String(s));
                }
            }
        }
        if !out.contains_key("source_id") {
            if let Some(id) = src.get("id").or(src.get("source_id")).or(src.get("sourceId")) {
                if let Some(s) = scalar_to_string(id.clone()) {
                    out.insert("source_id".to_string(), Value::String(s));
                }
            }
        }
    }

    for (from, to) in [
        ("source", "source_table"),
        ("table", "source_table"),
        ("entity_table", "source_table"),
        ("entity", "source_table"),
        ("source_type", "source_table"),
    ] {
        if !out.contains_key(to) {
            if let Some(v) = out.remove(from) {
                if let Some(s) = scalar_to_string(v) {
                    out.insert(to.to_string(), Value::String(s));
                }
            }
        }
    }

    for (from, to) in [
        ("document_id", "source_id"),
        ("business_document_id", "source_id"),
        ("photo_id", "source_id"),
        ("row_id", "source_id"),
        ("entity_id", "source_id"),
    ] {
        if !out.contains_key(to) {
            if let Some(v) = out.get(from).cloned() {
                if let Some(s) = scalar_to_string(v) {
                    out.insert(to.to_string(), Value::String(s));
                }
            }
        }
    }

    // Bare `id` when distinct from queue_id (common on reprocess/backfill payloads)
    if !out.contains_key("source_id") {
        if let Some(id) = out.get("id").cloned() {
            let id_s = scalar_to_string(id);
            let qid = out
                .get("queue_id")
                .and_then(|v| scalar_to_string(v.clone()));
            if id_s.as_deref() != qid.as_deref() {
                if let Some(s) = id_s {
                    out.insert("source_id".to_string(), Value::String(s));
                }
            }
        }
    }

    if !out.contains_key("source_table") {
        if let Some(t) = &batch.source_table {
            out.insert("source_table".to_string(), Value::String(t.clone()));
        }
    }
    if !out.contains_key("source_id") {
        if let Some(id) = &batch.source_id {
            out.insert("source_id".to_string(), Value::String(id.clone()));
        }
    }

    let action = out
        .get("action_type")
        .and_then(|v| v.as_str())
        .unwrap_or("mirror_file")
        .to_string();
    let is_mirror = action == "mirror_file";
    let has_signed = out.contains_key("signed_url");

    if is_mirror && !out.contains_key("source_table") && has_signed {
        out.insert(
            "source_table".to_string(),
            Value::String("business_documents".to_string()),
        );
        eprintln!("CCC_PACKAGE_CLAIM_INFER queue_id={:?} source_table=business_documents", out.get("queue_id"));
    }
    if action.as_str() == "delete_folder" && !out.contains_key("source_table") {
        out.insert(
            "source_table".to_string(),
            Value::String("business_intake_links".to_string()),
        );
    }
}

fn normalize_claim_item_value(v: serde_json::Value, batch: &BatchAckDefaults) -> serde_json::Value {
    use serde_json::{Map, Value};

    let Some(mut obj) = v.as_object().cloned() else {
        return v;
    };

    for nest_key in ["mirror_file", "payload", "job", "data", "mirror", "ack"] {
        if let Some(Value::Object(nested)) = obj.remove(nest_key) {
            for (k, val) in nested {
                obj.entry(k).or_insert(val);
            }
        }
    }

    let mut out = Map::new();
    for (k, val) in obj {
        let snake = camel_to_snake_field(&k).to_string();
        let val = if snake == "source_id" || snake == "queue_id" {
            scalar_to_string(val.clone())
                .map(Value::String)
                .unwrap_or(val)
        } else if snake == "source_table" {
            scalar_to_string(val.clone())
                .map(Value::String)
                .unwrap_or(val)
        } else {
            val
        };
        out.insert(snake, val);
    }

    if let Some(fh) = out.remove("filename_hint") {
        out.entry("filename".to_string()).or_insert(fh);
    }
    if let Some(ro) = out.remove("ro_folder") {
        out.entry("ro_folder_name".to_string()).or_insert(ro);
    }

    apply_ack_identity_defaults(&mut out, batch);

    Value::Object(out)
}

fn edge_functions_v1_base(backend_url: &str) -> Option<String> {
    let trimmed = backend_url.trim();
    let marker = "/functions/v1";
    let idx = trimmed.find(marker)?;
    Some(trimmed[..idx + marker.len()].to_string())
}

fn claim_url(base: &str) -> String {
    format!("{base}/ccc-package-claim-batch")
}

fn ack_url(base: &str) -> String {
    format!("{base}/ccc-package-ack")
}

fn build_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())
}

async fn post_json(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    body: &impl Serialize,
) -> Result<reqwest::Response, String> {
    client
        .post(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("apikey", token)
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))
}

fn queue_ack(
    pending: &mut Vec<PendingAck>,
    item: &ClaimItem,
    status: &str,
    error_message: Option<String>,
    written_path: Option<String>,
) {
    pending.push(PendingAck {
        queue_id: item.queue_id.clone(),
        source_table: item.source_table.clone(),
        source_id: item.source_id.clone(),
        status: status.to_string(),
        error_message,
        written_path,
        sub_folder: item
            .sub_folder
            .clone()
            .filter(|s| !s.trim().is_empty()),
        action_type: match item.action() {
            "mirror_file" => None,
            other => Some(other.to_string()),
        },
    });
}

async fn post_ack_batch(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    business_id: &str,
    device_id: &str,
    pending: &[PendingAck],
) {
    if pending.is_empty() {
        return;
    }

    if let Err(e) = crate::api_contracts::validate_package_ack_batch_request(
        business_id,
        device_id,
        pending.len(),
    ) {
        eprintln!("CCC_PACKAGE_ACK_CONTRACT_FAIL batch err={}", e);
        record_ack_fail(&format!("contract: {e}"));
        return;
    }

    for (i, ack) in pending.iter().enumerate() {
        if let Err(e) = crate::api_contracts::validate_package_ack_item(
            &ack.queue_id,
            &ack.status,
            &ack.source_table,
            &ack.source_id,
        ) {
            eprintln!(
                "CCC_PACKAGE_ACK_CONTRACT_FAIL item[{}] queue_id={} err={}",
                i, ack.queue_id, e
            );
            record_ack_fail(&format!("contract item[{i}]: {e}"));
            return;
        }
    }

    let body = AckBatchBody {
        business_id,
        device_id,
        items: pending
            .iter()
            .map(|a| AckItemBody {
                queue_id: &a.queue_id,
                source_table: &a.source_table,
                source_id: &a.source_id,
                status: &a.status,
                written_path: a.written_path.as_deref(),
                error_message: a.error_message.as_deref(),
                sub_folder: a.sub_folder.as_deref(),
                action_type: a.action_type.as_deref(),
            })
            .collect(),
    };

    eprintln!(
        "CCC_PACKAGE_ACK_POST business_id={} device_id={} items={}",
        business_id,
        device_id,
        pending.len()
    );

    match post_json(client, url, token, &body).await {
        Ok(resp) if resp.status().is_success() => {
            eprintln!(
                "CCC_PACKAGE_ACK_OK batch items={} ok={} fail={}",
                pending.len(),
                pending.iter().filter(|a| a.status == "ok").count(),
                pending.iter().filter(|a| a.status == "error").count()
            );
            for _ in 0..pending.len() {
                record_ack_ok();
            }
        }
        Ok(resp) => {
            let http = resp.status();
            let text = resp.text().await.unwrap_or_default();
            let preview: String = text.chars().take(300).collect();
            eprintln!("CCC_PACKAGE_ACK_HTTP batch http={} body={}", http, preview);
            record_ack_fail(&format!("HTTP {http}: {preview}"));
        }
        Err(e) => {
            eprintln!("CCC_PACKAGE_ACK_FAIL batch err={}", e);
            record_ack_fail(&format!("network: {e}"));
        }
    }
}

fn sanitize_path_segment(s: &str) -> Option<String> {
    let t = s.trim().trim_matches(|c| c == '/' || c == '\\');
    if t.is_empty() || t == ".." {
        return None;
    }
    let cleaned: String = t
        .chars()
        .map(|c| {
            if r#"<>:\"|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim_end_matches('.').to_string();
    if cleaned.is_empty() || cleaned == ".." {
        None
    } else {
        Some(cleaned)
    }
}

fn append_sub_folder(mut base: PathBuf, sub_folder: &str) -> Option<PathBuf> {
    for part in sub_folder.split(['/', '\\']) {
        let seg = sanitize_path_segment(part)?;
        base = base.join(seg);
    }
    Some(base)
}

/// Relative path under CCC Import root from `target_path_hint` (sanitized segments).
pub(crate) fn path_under_root(root: &str, hint: &str) -> Option<PathBuf> {
    append_sub_folder(PathBuf::from(root), hint)
}

/// Mirror writer path: `sub_folder` → `{root}/{ro}/{sub_folder}/{file}`; legacy `bucket`; else flat under RO.
pub(crate) fn destination_path_for_item(root: &str, item: &ClaimItem) -> PathBuf {
    let ro = item
        .ro_folder_name
        .as_deref()
        .and_then(sanitize_path_segment)
        .unwrap_or_else(|| "RO".to_string());
    let file = item
        .filename
        .as_deref()
        .and_then(sanitize_path_segment)
        .unwrap_or_else(|| "file".to_string());
    let mut base = PathBuf::from(root).join(ro);

    if let Some(sf) = item.sub_folder.as_deref().filter(|s| !s.trim().is_empty()) {
        if let Some(with_sub) = append_sub_folder(base.clone(), sf) {
            return with_sub.join(&file);
        }
    }

    if let Some(bucket) = item
        .bucket
        .as_deref()
        .and_then(sanitize_path_segment)
    {
        base = base.join(bucket);
    }

    base.join(file)
}

fn destination_path(root: &str, item: &ClaimItem) -> PathBuf {
    destination_path_for_item(root, item)
}

#[cfg(test)]
mod path_tests {
    use super::*;

    fn item(
        ro: &str,
        sub_folder: Option<&str>,
        bucket: &str,
        file: &str,
    ) -> ClaimItem {
        ClaimItem {
            queue_id: "q".to_string(),
            action_type: None,
            ro_folder_name: Some(ro.to_string()),
            sub_folder: sub_folder.map(String::from),
            bucket: if bucket.is_empty() {
                None
            } else {
                Some(bucket.to_string())
            },
            signed_url: Some("https://example.com/x".to_string()),
            filename: Some(file.to_string()),
            target_path_hint: None,
            source_table: "t".to_string(),
            source_id: "id".to_string(),
        }
    }

    #[test]
    fn uses_sub_folder_when_present() {
        let p = destination_path_for_item(
            r"C:\FileWisely\CCC Import",
            &item("RO1", Some("photos/check_in"), "", "pic.jpg"),
        );
        let s = p.to_string_lossy();
        assert!(s.contains("RO1"));
        assert!(s.contains("photos"));
        assert!(s.contains("check_in"));
        assert!(s.ends_with("pic.jpg"));
    }

    #[test]
    fn legacy_bucket_when_no_sub_folder() {
        let p = destination_path_for_item(
            r"C:\FileWisely\CCC Import",
            &item("RO1", None, "estimate", "doc.pdf"),
        );
        let s = p.to_string_lossy();
        assert!(s.contains("estimate"));
        assert!(s.ends_with("doc.pdf"));
    }

    #[test]
    fn flat_when_no_sub_folder_or_bucket() {
        let p = destination_path_for_item(
            r"C:\FileWisely\CCC Import",
            &item("RO1", None, "", "only.pdf"),
        );
        let s = p.to_string_lossy();
        assert!(s.ends_with(r"RO1\only.pdf") || s.ends_with("RO1/only.pdf"));
    }

    #[test]
    fn delete_folder_resolves_under_root() {
        let p = path_under_root(r"C:\FileWisely\CCC Import", "RO1_Smith").unwrap();
        let s = p.to_string_lossy();
        assert!(s.contains("RO1_Smith"));
        assert!(s.contains("CCC Import"));
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    #[test]
    fn parses_snake_case_items() {
        let body = r#"{"items":[{"queue_id":"550e8400-e29b-41d4-a716-446655440000","ro_folder":"RO1","signed_url":"https://x.test/a.jpg","filename_hint":"a.jpg","source_table":"business_documents","source_id":"42"}]}"#;
        let items = parse_claim_batch_body(body).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].ro_folder_name.as_deref(), Some("RO1"));
    }

    #[test]
    fn parses_camel_case_and_nested_mirror_file() {
        let body = r#"{"items":[{"queueId":"550e8400-e29b-41d4-a716-446655440000","actionType":"mirror_file","mirror_file":{"roFolder":"RO2","signedUrl":"https://x.test/b.jpg","filenameHint":"b.jpg"},"sourceTable":"business_documents","sourceId":99}]}"#;
        let items = parse_claim_batch_body(body).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].filename.as_deref(), Some("b.jpg"));
        assert_eq!(items[0].source_id, "99");
    }

    #[test]
    fn infers_source_table_for_mirror_without_ack_fields() {
        let body = r#"{"items":[{"queue_id":"550e8400-e29b-41d4-a716-446655440000","ro_folder":"RO1","signed_url":"https://x.test/a.jpg","filename_hint":"a.jpg","document_id":"doc-uuid-1"}]}"#;
        let items = parse_claim_batch_body(body).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_table, "business_documents");
        assert_eq!(items[0].source_id, "doc-uuid-1");
    }

    #[test]
    fn parses_compact_source_object() {
        let body = r#"{"items":[{"queue_id":"550e8400-e29b-41d4-a716-446655440001","signed_url":"https://x.test/c.jpg","filename":"c.jpg","source":{"table":"business_documents","id":"99"}}]}"#;
        let items = parse_claim_batch_body(body).unwrap();
        assert_eq!(items[0].source_table, "business_documents");
        assert_eq!(items[0].source_id, "99");
    }

    #[test]
    fn fails_when_jobs_present_but_unparseable() {
        let body = r#"{"items":[{"queueId":"x","signed_url":"https://x.test/d.jpg"}]}"#;
        assert!(parse_claim_batch_body(body).is_err());
    }
}

async fn download_to_path(client: &reqwest::Client, url: &str, dest: &PathBuf) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    let status = resp.status();
    if status == StatusCode::FORBIDDEN || status == StatusCode::GONE {
        return Err("url_expired".to_string());
    }
    if !status.is_success() {
        return Err(format!("download HTTP {}", status));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("read body: {e}"))?;
    std::fs::write(dest, &bytes).map_err(|e| classify_write_error(&e))?;
    Ok(())
}

fn classify_write_error(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::PermissionDenied => "permission_denied".to_string(),
        io::ErrorKind::StorageFull => "disk_full".to_string(),
        _ => e.to_string(),
    }
}

fn ensure_path_under_root(root: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
    let root_canon = std::fs::canonicalize(root).map_err(|e| format!("root canonicalize: {e}"))?;
    let target_canon = if target.exists() {
        std::fs::canonicalize(target).map_err(|e| format!("target canonicalize: {e}"))?
    } else {
        let mut acc = root_canon.clone();
        let rel = target
            .strip_prefix(root)
            .map_err(|_| "target outside ccc import root".to_string())?;
        for comp in rel.components() {
            use std::path::Component;
            match comp {
                Component::Normal(p) => acc.push(p),
                Component::CurDir => {}
                _ => return Err("invalid path segment".to_string()),
            }
        }
        acc
    };
    if !target_canon.starts_with(&root_canon) {
        return Err("path outside ccc import root".to_string());
    }
    Ok(())
}

fn delete_folder_under_root(root: &str, target_path_hint: &str) -> Result<(), String> {
    let hint = target_path_hint.trim();
    if hint.is_empty() {
        return Err("missing target_path_hint".to_string());
    }
    let Some(dir) = path_under_root(root, hint) else {
        return Err("invalid target_path_hint".to_string());
    };
    let root_path = std::path::Path::new(root);
    ensure_path_under_root(root_path, &dir)?;
    if !dir.exists() {
        return Ok(());
    }
    if !dir.is_dir() {
        return Err("target_not_a_directory".to_string());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| classify_write_error(&e))
}

async fn process_delete_folder(root: &str, item: &ClaimItem, pending: &mut Vec<PendingAck>) {
    let hint = match item.target_path_hint.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(h) => h,
        None => {
            note_write_error("missing target_path_hint");
            queue_ack(
                pending,
                item,
                "error",
                Some("missing target_path_hint".to_string()),
                None,
            );
            return;
        }
    };
    match delete_folder_under_root(root, hint) {
        Ok(()) => {
            eprintln!(
                "CCC_PACKAGE_DELETE_OK queue_id={} target={}",
                item.queue_id, hint
            );
            queue_ack(pending, item, "ok", None, None);
            note_write_ok();
        }
        Err(msg) => {
            eprintln!(
                "CCC_PACKAGE_DELETE_FAIL queue_id={} target={} err={}",
                item.queue_id, hint, msg
            );
            note_write_error(&msg);
            queue_ack(pending, item, "error", Some(msg), None);
        }
    }
}

async fn process_mirror(
    client: &reqwest::Client,
    root: &str,
    item: &ClaimItem,
    pending: &mut Vec<PendingAck>,
) {
    let Some(url) = item.signed_url.as_deref().filter(|s| !s.trim().is_empty()) else {
        note_write_error("missing signed_url");
        queue_ack(
            pending,
            item,
            "error",
            Some("missing signed_url".to_string()),
            None,
        );
        return;
    };
    let dest = destination_path(root, item);
    match download_to_path(client, url, &dest).await {
        Ok(()) => {
            let path = dest.display().to_string();
            eprintln!(
                "CCC_PACKAGE_WRITTEN queue_id={} path={}",
                item.queue_id, path
            );
            queue_ack(pending, item, "ok", None, Some(path));
            note_write_ok();
        }
        Err(msg) if msg == "url_expired" => {
            note_write_error("url_expired");
            queue_ack(
                pending,
                item,
                "error",
                Some("url_expired".to_string()),
                None,
            );
        }
        Err(msg) => {
            eprintln!(
                "CCC_PACKAGE_WRITE_FAIL queue_id={} err={}",
                item.queue_id, msg
            );
            note_write_error(&msg);
            queue_ack(pending, item, "error", Some(msg), None);
        }
    }
}

async fn process_item(
    client: &reqwest::Client,
    root: &str,
    item: &ClaimItem,
    pending: &mut Vec<PendingAck>,
) {
    match item.action() {
        "delete_folder" => process_delete_folder(root, item, pending).await,
        "delete_file" | "archive_folder" => {
            let msg = format!("unsupported action: {}", item.action());
            eprintln!(
                "CCC_PACKAGE_UNSUPPORTED queue_id={} action={}",
                item.queue_id,
                item.action()
            );
            queue_ack(pending, item, "error", Some(msg), None);
        }
        _ => process_mirror(client, root, item, pending).await,
    }
}

async fn poll_once(app: &AppHandle) {
    if !ccc_import_writable() {
        set_offline(
            true,
            Some("CCC Import folder not writable — check path exists and is not blocked by AV"),
        );
        crate::device_health::refresh_tray(app);
        return;
    }

    let tenant = match tenant_config::load_tenant_config(app) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[UCE] ccc package sync: tenant: {e}");
            set_offline(true, Some(&format!("tenant config: {e}")));
            crate::device_health::refresh_tray(app);
            return;
        }
    };

    let business_id = tenant.business_id.trim();
    let backend = tenant.backend_url.trim();
    let token = tenant.anon_key.trim();
    if business_id.is_empty() {
        set_offline(true, Some("missing business_id in uce-tenant.json"));
        crate::device_health::refresh_tray(app);
        return;
    }
    if backend.is_empty() || token.is_empty() {
        set_offline(true, Some("missing backend_url or anon_key"));
        crate::device_health::refresh_tray(app);
        return;
    }

    let settings = ccc_import_settings::load_settings(app);
    let Some(root) = effective_ccc_package_root(&settings) else {
        set_offline(true, Some("ccc_package_root not configured in settings.json"));
        crate::device_health::refresh_tray(app);
        return;
    };

    let Some(base) = edge_functions_v1_base(backend) else {
        eprintln!("[UCE] ccc package sync: cannot derive functions/v1 base from backend_url");
        set_offline(
            true,
            Some("backend_url must contain /functions/v1 for claim-batch"),
        );
        crate::device_health::refresh_tray(app);
        return;
    };

    let device_id = match device_id::load_device_id(app) {
        Some(id) => id,
        None => {
            set_offline(
                true,
                Some("device_id not set — open UCE overlay to complete Connect"),
            );
            crate::device_health::refresh_tray(app);
            return;
        }
    };

    let client = match build_http_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[UCE] ccc package sync: http client: {e}");
            set_offline(true, Some(&format!("http client: {e}")));
            crate::device_health::refresh_tray(app);
            return;
        }
    };

    if let Err(e) =
        crate::api_contracts::validate_claim_batch_request(business_id, &device_id, CLAIM_LIMIT)
    {
        eprintln!("CCC_PACKAGE_CLAIM_CONTRACT_FAIL {e}");
        set_offline(true, Some(&e));
        crate::device_health::refresh_tray(app);
        return;
    }

    let claim_endpoint = claim_url(&base);
    let claim_body = ClaimBatchBody {
        business_id,
        device_id: &device_id,
        limit: CLAIM_LIMIT,
    };

    let claim_resp = match post_json(&client, &claim_endpoint, token, &claim_body).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("CCC_PACKAGE_CLAIM_NETWORK err={}", e);
            set_offline(true, Some(&format!("claim network: {e}")));
            crate::device_health::refresh_tray(app);
            return;
        }
    };

    let http_status = claim_resp.status();
    let claim_body_text = claim_resp.text().await.unwrap_or_default();

    if !http_status.is_success() {
        eprintln!(
            "CCC_PACKAGE_CLAIM_HTTP http={} body={}",
            http_status,
            claim_body_text.chars().take(200).collect::<String>()
        );
        set_offline(
            true,
            Some(&format!(
                "claim HTTP {http_status}: {}",
                claim_body_text.chars().take(200).collect::<String>()
            )),
        );
        crate::device_health::refresh_tray(app);
        return;
    }

    set_offline(false, None);
    crate::device_health::note_ccc_sync_activity();

    let items = match parse_claim_batch_body(&claim_body_text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("CCC_PACKAGE_CLAIM_PARSE err={}", e);
            note_claim_error(&e);
            LAST_CLAIM_BATCH_COUNT.store(0, Ordering::Relaxed);
            crate::device_health::refresh_tray(app);
            return;
        }
    };

    let count = items.len() as u32;
    LAST_CLAIM_BATCH_COUNT.store(count, Ordering::Relaxed);
    LAST_CLAIM_PARSED_MS.store(now_unix_ms(), Ordering::Relaxed);
    eprintln!("CCC_PACKAGE_CLAIM_OK parsed_items={}", count);

    if items.is_empty() {
        SYNCING_COUNT.store(0, Ordering::Relaxed);
        crate::device_health::refresh_tray(app);
        return;
    }

    let ack_endpoint = ack_url(&base);
    clear_ack_batch_counters();
    SYNCING_COUNT.store(count, Ordering::Relaxed);
    crate::device_health::refresh_tray(app);

    let mut pending_acks = Vec::with_capacity(items.len());
    for item in &items {
        process_item(&client, &root, item, &mut pending_acks).await;
    }

    post_ack_batch(
        &client,
        &ack_endpoint,
        token,
        business_id,
        &device_id,
        &pending_acks,
    )
    .await;

    SYNCING_COUNT.store(0, Ordering::Relaxed);
    crate::device_health::refresh_tray(app);
}

async fn wait_for_device_id(app: &AppHandle, max_wait_secs: u64) {
    let deadline = std::time::Instant::now() + Duration::from_secs(max_wait_secs);
    while std::time::Instant::now() < deadline {
        if device_id::load_device_id(app).is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    eprintln!("CCC_PACKAGE_SYNC_DEVICE_ID_WAIT_TIMEOUT secs={max_wait_secs}");
    let _ = app.emit("uce:request-device-id-sync", ());
}

pub fn spawn_ccc_package_sync(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        wait_for_device_id(&app, 45).await;
        loop {
            if !is_sync_paused() {
                poll_once(&app).await;
            }
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
        }
    });
}
