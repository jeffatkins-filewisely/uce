//! Ring-buffer trace for Connection Doctor: detection → copy → emit → upload.

use serde::Serialize;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const RING_MAX: usize = 24;

#[derive(Clone, Serialize)]
pub struct PipelineStageEntry {
    pub at_unix_ms: i64,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn push(
    ring: &Mutex<VecDeque<PipelineStageEntry>>,
    path: impl Into<String>,
    detail: Option<String>,
) {
    let entry = PipelineStageEntry {
        at_unix_ms: now_ms(),
        path: path.into(),
        detail,
    };
    if let Ok(mut q) = ring.lock() {
        q.push_back(entry);
        while q.len() > RING_MAX {
            q.pop_front();
        }
    }
}

static LAST_DETECTED: LazyLock<Mutex<VecDeque<PipelineStageEntry>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));
static LAST_REJECTED: LazyLock<Mutex<VecDeque<PipelineStageEntry>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));
static LAST_ACCEPTED: LazyLock<Mutex<VecDeque<PipelineStageEntry>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));
static LAST_COPY_ATTEMPTS: LazyLock<Mutex<VecDeque<PipelineStageEntry>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));
static LAST_COPY_FAILURES: LazyLock<Mutex<VecDeque<PipelineStageEntry>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));
static LAST_COPY_SUCCESS: LazyLock<Mutex<VecDeque<PipelineStageEntry>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));
static LAST_EMITTED_INCOMING: LazyLock<Mutex<VecDeque<PipelineStageEntry>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));
static LAST_UPLOAD_ATTEMPTS: LazyLock<Mutex<VecDeque<PipelineStageEntry>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));
static LAST_UPLOAD_FAILURES: LazyLock<Mutex<VecDeque<PipelineStageEntry>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));
static LAST_UPLOAD_SUCCESSES: LazyLock<Mutex<VecDeque<PipelineStageEntry>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

pub fn record_detected(path: &str) {
    push(&LAST_DETECTED, path, None);
}

pub fn record_rejected(path: &str, reason: &str) {
    push(&LAST_REJECTED, path, Some(reason.to_string()));
}

pub fn record_accepted(path: &str, detail: Option<String>) {
    push(&LAST_ACCEPTED, path, detail);
}

pub fn record_copy_attempt(path: &str) {
    push(&LAST_COPY_ATTEMPTS, path, None);
}

pub fn record_copy_success(staged_path: &str) {
    push(&LAST_COPY_SUCCESS, staged_path, None);
}

pub fn record_copy_failure(path: &str, err: &str) {
    push(&LAST_COPY_FAILURES, path, Some(err.to_string()));
}

pub fn record_emit_incoming(path: &str) {
    push(&LAST_EMITTED_INCOMING, path, None);
}

pub fn record_upload_attempt(path: &str) {
    push(&LAST_UPLOAD_ATTEMPTS, path, None);
}

pub fn record_upload_success(path: &str) {
    push(&LAST_UPLOAD_SUCCESSES, path, None);
}

pub fn record_upload_failure(path: &str, detail: &str) {
    push(&LAST_UPLOAD_FAILURES, path, Some(detail.to_string()));
}

fn collect(ring: &Mutex<VecDeque<PipelineStageEntry>>) -> Vec<PipelineStageEntry> {
    ring.lock()
        .map(|q| q.iter().cloned().collect())
        .unwrap_or_default()
}

pub fn snapshot_json() -> serde_json::Value {
    json!({
        "last_detected_files": collect(&LAST_DETECTED),
        "last_rejected_files": collect(&LAST_REJECTED),
        "last_accepted_files": collect(&LAST_ACCEPTED),
        "last_copy_attempts": collect(&LAST_COPY_ATTEMPTS),
        "last_copy_successes": collect(&LAST_COPY_SUCCESS),
        "last_copy_failures": collect(&LAST_COPY_FAILURES),
        "last_emitted_incoming_files": collect(&LAST_EMITTED_INCOMING),
        "last_upload_attempts": collect(&LAST_UPLOAD_ATTEMPTS),
        "last_upload_successes": collect(&LAST_UPLOAD_SUCCESSES),
        "last_upload_failures": collect(&LAST_UPLOAD_FAILURES),
    })
}
