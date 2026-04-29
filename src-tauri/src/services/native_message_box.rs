//! Single entry point for Windows `MessageBoxW` — **every** automatic native dialog must go through here.
//! Applies `popup_suppression::suppress_all_effective()` and records Connection Doctor fields.

use super::js_runtime_diag;
use super::popup_suppression;

/// Style bits: `MB_OK`, `MB_ICONWARNING`, `MB_ICONINFORMATION`, `MB_SETFOREGROUND` from `windows_sys`.
#[cfg(windows)]
pub fn uce_show_native_message_box(
    kind: &str,
    source: &str,
    title: &str,
    body: &str,
    mb_flags: u32,
) {
    if suppress_all_native(kind, source, title, body) {
        return;
    }
    js_runtime_diag::record_native_popup_shown(
        kind.to_string(),
        source.to_string(),
        summarize_native_dialog(title, body),
    );
    eprintln!(
        "UCE_UI_NATIVE_ALERT kind={} source={} title=\"{}\"",
        kind, source, title
    );
    unsafe_raw_message_box(title, body, mb_flags);
}

#[cfg(not(windows))]
pub fn uce_show_native_message_box(
    kind: &str,
    source: &str,
    title: &str,
    body: &str,
    _mb_flags: u32,
) {
    let _ = (kind, source, title, body);
    eprintln!(
        "[UCE] native MessageBox skipped (not Windows): kind={kind} source={source}"
    );
}

fn summarize_native_dialog(title: &str, body: &str) -> String {
    let b = body.chars().take(400).collect::<String>();
    format!("{} | {}", title, b)
}

/// Returns `true` if suppressed (caller must not show MessageBox).
fn suppress_all_native(kind: &str, source: &str, title: &str, body: &str) -> bool {
    // Escape hatch: force native dialogs visible even when global suppression is on (desktop debugging).
    if std::env::var("UCE_ALLOW_NATIVE_MESSAGEBOX")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return false;
    }
    if !popup_suppression::suppress_all_effective() {
        return false;
    }
    let preview = summarize_native_dialog(title, body);
    eprintln!(
        "UCE_NATIVE_POPUP_SUPPRESSED kind={} source={} message_preview={}",
        kind,
        source,
        preview.chars().take(200).collect::<String>()
    );
    js_runtime_diag::record_native_popup_suppressed(
        kind.to_string(),
        source.to_string(),
        preview,
    );
    true
}

#[cfg(windows)]
fn unsafe_raw_message_box(title: &str, body: &str, mb_flags: u32) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;
    let title_w: Vec<u16> = OsStr::new(title)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let body_w: Vec<u16> = OsStr::new(body)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body_w.as_ptr(),
            title_w.as_ptr(),
            mb_flags,
        );
    }
}
