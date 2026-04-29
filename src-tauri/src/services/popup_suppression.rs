//! Global kill-switch for user-visible popups (MessageBox, toasts, alerts) — diagnostics only.
//! Native Windows dialogs must use [`super::native_message_box::uce_show_native_dialog`] /
//! [`super::native_message_box::uce_show_native_message_box`] only.

use std::sync::atomic::{AtomicBool, Ordering};

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
