//! Claim → download → write → ack loop for FileWisely CCC package queue (15s when online).

use crate::ccc_import_settings::{self, effective_ccc_package_root};
use crate::device_id;
use crate::tenant_config;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::menu::MenuItem;
use tauri::{AppHandle, Wry};

const POLL_INTERVAL_SECS: u64 = 15;
const CLAIM_LIMIT: u32 = 25;
const HTTP_TIMEOUT_SECS: u64 = 120;

static SYNCING_COUNT: AtomicU32 = AtomicU32::new(0);
static OFFLINE: Mutex<bool> = Mutex::new(false);
static CCC_SYNC_STATUS_ITEM: OnceLock<MenuItem<Wry>> = OnceLock::new();

/// Clone stored when the tray menu is built (`main.rs`).
pub fn register_ccc_sync_status_item(item: MenuItem<Wry>) {
    let _ = CCC_SYNC_STATUS_ITEM.set(item);
}

#[derive(Debug, Deserialize)]
struct ClaimBatchResponse {
    #[serde(default)]
    items: Vec<ClaimItem>,
}

#[derive(Debug, Clone, Deserialize)]
struct ClaimItem {
    queue_id: String,
    ro_folder_name: String,
    bucket: String,
    signed_url: String,
    filename: String,
    source_table: String,
    source_id: String,
}

#[derive(Serialize)]
struct ClaimBatchBody<'a> {
    device_id: &'a str,
    limit: u32,
}

#[derive(Serialize)]
struct AckBody<'a> {
    queue_id: &'a str,
    source_table: &'a str,
    source_id: &'a str,
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_message: Option<&'a str>,
}

pub fn tray_ccc_sync_label() -> String {
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

fn set_offline(offline: bool) {
    if let Ok(mut g) = OFFLINE.lock() {
        *g = offline;
    }
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

async fn ack_item(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    item: &ClaimItem,
    status: &str,
    error_message: Option<&str>,
) {
    let body = AckBody {
        queue_id: &item.queue_id,
        source_table: &item.source_table,
        source_id: &item.source_id,
        status,
        error_message,
    };
    match post_json(client, url, token, &body).await {
        Ok(resp) if resp.status().is_success() => {
            eprintln!(
                "CCC_PACKAGE_ACK_OK queue_id={} status={}",
                item.queue_id, status
            );
        }
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            eprintln!(
                "CCC_PACKAGE_ACK_HTTP queue_id={} http={} body={}",
                item.queue_id,
                status,
                text.chars().take(200).collect::<String>()
            );
        }
        Err(e) => {
            eprintln!(
                "CCC_PACKAGE_ACK_FAIL queue_id={} err={}",
                item.queue_id, e
            );
        }
    }
}

fn destination_path(root: &str, item: &ClaimItem) -> PathBuf {
    PathBuf::from(root)
        .join(&item.ro_folder_name)
        .join(&item.bucket)
        .join(&item.filename)
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

async fn process_item(
    client: &reqwest::Client,
    ack_endpoint: &str,
    token: &str,
    root: &str,
    item: &ClaimItem,
) {
    let dest = destination_path(root, item);
    match download_to_path(client, &item.signed_url, &dest).await {
        Ok(()) => {
            eprintln!(
                "CCC_PACKAGE_WRITTEN queue_id={} path={}",
                item.queue_id,
                dest.display()
            );
            ack_item(client, ack_endpoint, token, item, "ok", None).await;
        }
        Err(msg) if msg == "url_expired" => {
            ack_item(
                client,
                ack_endpoint,
                token,
                item,
                "error",
                Some("url_expired"),
            )
            .await;
        }
        Err(msg) => {
            eprintln!(
                "CCC_PACKAGE_WRITE_FAIL queue_id={} err={}",
                item.queue_id, msg
            );
            ack_item(
                client,
                ack_endpoint,
                token,
                item,
                "error",
                Some(&msg),
            )
            .await;
        }
    }
}

async fn poll_once(app: &AppHandle) {
    let tenant = match tenant_config::load_tenant_config(app) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[UCE] ccc package sync: tenant: {e}");
            set_offline(true);
            refresh_tray_ccc_sync_item(app);
            return;
        }
    };

    let backend = tenant.backend_url.trim();
    let token = tenant.anon_key.trim();
    if backend.is_empty() || token.is_empty() {
        set_offline(true);
        refresh_tray_ccc_sync_item(app);
        return;
    }

    let settings = ccc_import_settings::load_settings(app);
    let Some(root) = effective_ccc_package_root(&settings) else {
        set_offline(true);
        refresh_tray_ccc_sync_item(app);
        return;
    };

    let Some(base) = edge_functions_v1_base(backend) else {
        eprintln!("[UCE] ccc package sync: cannot derive functions/v1 base from backend_url");
        set_offline(true);
        refresh_tray_ccc_sync_item(app);
        return;
    };

    let device_id = match device_id::load_device_id(app) {
        Some(id) => id,
        None => {
            set_offline(true);
            refresh_tray_ccc_sync_item(app);
            return;
        }
    };

    let client = match build_http_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[UCE] ccc package sync: http client: {e}");
            set_offline(true);
            refresh_tray_ccc_sync_item(app);
            return;
        }
    };

    let claim_endpoint = claim_url(&base);
    let claim_body = ClaimBatchBody {
        device_id: &device_id,
        limit: CLAIM_LIMIT,
    };

    let claim_resp = match post_json(&client, &claim_endpoint, token, &claim_body).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("CCC_PACKAGE_CLAIM_NETWORK err={}", e);
            set_offline(true);
            refresh_tray_ccc_sync_item(app);
            return;
        }
    };

    if !claim_resp.status().is_success() {
        let status = claim_resp.status();
        let text = claim_resp.text().await.unwrap_or_default();
        eprintln!(
            "CCC_PACKAGE_CLAIM_HTTP http={} body={}",
            status,
            text.chars().take(200).collect::<String>()
        );
        set_offline(true);
        refresh_tray_ccc_sync_item(app);
        return;
    }

    set_offline(false);

    let batch: ClaimBatchResponse = match claim_resp.json().await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("CCC_PACKAGE_CLAIM_PARSE err={}", e);
            refresh_tray_ccc_sync_item(app);
            return;
        }
    };

    if batch.items.is_empty() {
        SYNCING_COUNT.store(0, Ordering::Relaxed);
        refresh_tray_ccc_sync_item(app);
        return;
    }

    let ack_endpoint = ack_url(&base);
    let count = batch.items.len() as u32;
    SYNCING_COUNT.store(count, Ordering::Relaxed);
    refresh_tray_ccc_sync_item(app);

    for item in &batch.items {
        process_item(&client, &ack_endpoint, token, &root, item).await;
    }

    SYNCING_COUNT.store(0, Ordering::Relaxed);
    refresh_tray_ccc_sync_item(app);
}

pub fn spawn_ccc_package_sync(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            poll_once(&app).await;
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
        }
    });
}
