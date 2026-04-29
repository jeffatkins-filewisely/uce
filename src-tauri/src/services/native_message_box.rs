//! Global native dialog entry — **all** Windows `MessageBoxW` usage must go through this module.
//! Prefer [`uce_show_native_dialog`] / [`uce_show_native_dialog_flags`]; [`uce_show_native_message_box`]
//! remains for title/body/flags call sites.
//!
//! For every attempt: `js_runtime_diag` **attempt** fields + `popup.log` `ATTEMPT` line **first**,
//! then release/suppression gates, then `SHOWN` / `SUPPRESSED` / `BLOCKED` + optional `MessageBoxW`.
//!
//! **Release builds:** native `MessageBoxW` is **disabled entirely** (log only). Opt-in for QA:
//! `UCE_ALLOW_NATIVE_MESSAGEBOX=1`.

use super::js_runtime_diag;
use super::popup_log;
use super::popup_suppression;

/// Default flags: OK + warning icon + foreground.
const MB_OK: u32 = 0;
const MB_ICONWARNING: u32 = 0x30;
const MB_ICONINFORMATION: u32 = 0x40;
const MB_SETFOREGROUND: u32 = 0x0001_0000;

pub const UCE_MB_WARNING_FOREGROUND: u32 = MB_OK | MB_ICONWARNING | MB_SETFOREGROUND;
pub const UCE_MB_INFO_FOREGROUND: u32 = MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND;

/// Single-string native dialog: optional title then body, separated by the first `\r\n\r\n` or `\n\n`.
/// If there is no separator, title is `"UCE"` and the whole string is the body.
pub fn uce_show_native_dialog(kind: &str, source: &str, message: &str) {
    uce_show_native_dialog_flags(kind, source, message, UCE_MB_WARNING_FOREGROUND);
}

pub fn uce_show_native_dialog_flags(kind: &str, source: &str, message: &str, mb_flags: u32) {
    let (title, body) = split_title_and_body(message);
    uce_show_native_message_box(kind, source, title, body, mb_flags);
}

fn split_title_and_body(message: &str) -> (&str, &str) {
    for sep in ["\r\n\r\n", "\n\n"] {
        if let Some(idx) = message.find(sep) {
            let title = message[..idx].trim_end();
            let body = message[idx + sep.len()..].trim_start();
            if !title.is_empty() {
                return (title, body);
            }
        }
    }
    ("UCE", message)
}

fn summarize_native_dialog(title: &str, body: &str) -> String {
    let b = body.chars().take(400).collect::<String>();
    format!("{} | {}", title, b)
}

/// Style bits: `MB_OK`, `MB_ICONWARNING`, etc. from `windows_sys`.
#[cfg(windows)]
pub fn uce_show_native_message_box(
    kind: &str,
    source: &str,
    title: &str,
    body: &str,
    mb_flags: u32,
) {
    let preview = summarize_native_dialog(title, body);
    record_attempt(kind, source, &preview);

    #[cfg(not(debug_assertions))]
    {
        let allow = std::env::var("UCE_ALLOW_NATIVE_MESSAGEBOX")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if !allow {
            let msg_line = format!("BLOCKED_PRODUCTION_NO_MESSAGEBOX {}", preview);
            popup_log::append(kind, source, &msg_line);
            let stderr_msg = preview.chars().take(500).collect::<String>();
            eprintln!(
                "UCE_NATIVE_POPUP_BLOCKED kind={} source={} message={}",
                kind, source, stderr_msg
            );
            js_runtime_diag::record_native_popup_suppressed(
                kind.to_string(),
                source.to_string(),
                format!("production_block|{}", preview),
            );
            let _ = mb_flags;
            return;
        }
    }

    if suppress_all_native(kind, source, &preview) {
        return;
    }
    let msg_line = format!("SHOWN {}", preview);
    popup_log::append(kind, source, &msg_line);
    let stderr_msg = preview.chars().take(500).collect::<String>();
    eprintln!(
        "UCE_NATIVE_POPUP kind={} source={} message={}",
        kind, source, stderr_msg
    );
    js_runtime_diag::record_native_popup_shown(
        kind.to_string(),
        source.to_string(),
        preview.clone(),
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
    let preview = summarize_native_dialog(title, body);
    record_attempt(kind, source, &preview);
    popup_log::append(
        kind,
        source,
        &format!("SKIPPED_NON_WINDOWS {}", preview),
    );
    eprintln!(
        "UCE_NATIVE_POPUP_SKIPPED_PLATFORM kind={kind} source={source} message={preview}"
    );
}

fn record_attempt(kind: &str, source: &str, preview: &str) {
    js_runtime_diag::record_native_popup_attempt(
        kind.to_string(),
        source.to_string(),
        preview.to_string(),
    );
    popup_log::append(kind, source, &format!("ATTEMPT {}", preview));
}

/// Returns `true` if suppressed (caller must not show MessageBox).
fn suppress_all_native(kind: &str, source: &str, preview: &str) -> bool {
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
    let msg_line = format!("SUPPRESSED {}", preview);
    popup_log::append(kind, source, &msg_line);
    let stderr_msg = preview.chars().take(500).collect::<String>();
    eprintln!(
        "UCE_NATIVE_POPUP_SUPPRESSED kind={} source={} message={}",
        kind, source, stderr_msg
    );
    js_runtime_diag::record_native_popup_suppressed(
        kind.to_string(),
        source.to_string(),
        preview.to_string(),
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
