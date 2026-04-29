//! Tracks whether the folder / PDF print watcher thread started successfully (debouncer + roots).

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

const NOT_INITIALIZED: u8 = 0;
const RUNNING: u8 = 1;
const FAILED: u8 = 2;

static STATUS: AtomicU8 = AtomicU8::new(NOT_INITIALIZED);

static LAST_FAILURE: Mutex<Option<String>> = Mutex::new(None);

pub fn status_label() -> &'static str {
    match STATUS.load(Ordering::SeqCst) {
        RUNNING => "running",
        FAILED => "failed",
        _ => "not_initialized",
    }
}

/// True after the print watcher / debouncer thread reports a healthy start.
pub fn is_watcher_running() -> bool {
    STATUS.load(Ordering::SeqCst) == RUNNING
}

pub fn set_running() {
    STATUS.store(RUNNING, Ordering::SeqCst);
    if let Ok(mut g) = LAST_FAILURE.lock() {
        *g = None;
    }
}

pub fn set_failed(reason: String) {
    STATUS.store(FAILED, Ordering::SeqCst);
    if let Ok(mut g) = LAST_FAILURE.lock() {
        *g = Some(reason);
    }
}

pub fn failure_reason() -> Option<String> {
    LAST_FAILURE.lock().ok().and_then(|g| (*g).clone())
}
