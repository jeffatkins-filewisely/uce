//! Global kill-switch for user-visible popups (MessageBox, toasts, alerts) — diagnostics only.

use std::sync::atomic::{AtomicBool, Ordering};

use super::js_runtime_diag;

static SUPPRESS_ALL: AtomicBool = AtomicBool::new(false);

/// Call once at app startup (release defaults to suppress until JS sync refines).
pub fn init_defaults_for_build_profile() {
    SUPPRESS_ALL.store(cfg!(not(debug_assertions)), Ordering::SeqCst);
}

pub fn set_from_js(v: bool) {
    SUPPRESS_ALL.store(v, Ordering::SeqCst);
}

/// `UCE_SUPPRESS_ALL_POPUPS` overrides JS/localStorage: `0`/`false` = never suppress, `1`/`true` = always suppress.
pub fn suppress_all_effective() -> bool {
    match std::env::var("UCE_SUPPRESS_ALL_POPUPS") {
        Ok(v) => {
            let t = v.trim();
            if t == "0" || t.eq_ignore_ascii_case("false") {
                return false;
            }
            if t == "1" || t.eq_ignore_ascii_case("true") {
                return true;
            }
        }
        Err(_) => {}
    }
    SUPPRESS_ALL.load(Ordering::SeqCst)
}

/// Returns `true` if the native dialog must **not** be shown.
pub fn guard_native_message_box(kind: &str, source: &str, message_hint: &str) -> bool {
    if !suppress_all_effective() {
        return false;
    }
    eprintln!(
        "UCE_UI_POPUP_SUPPRESSED kind={} source={} message={}",
        kind, source, message_hint
    );
    js_runtime_diag::record_popup_suppressed(
        kind.to_string(),
        source.to_string(),
        Some(message_hint.to_string()),
    );
    true
}
