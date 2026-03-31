//! CCC **Change Requests** flow: after a PDF is claimed from `%LOCALAPPDATA%\\Temp\\CCC`,
//! optionally close only the Word window correlated with that flow and restore CCC focus.
//!
//! Enabled when [`crate::config::print_config::ccc_temp_watch_only`] is true, unless
//! `UCE_CCC_CR_AUTOCLOSE_WORD=0`. Strong correlation uses the document path parsed from the
//! Word title (same folder + same stem as the PDF). Optional weak mode:
//! `UCE_CCC_CR_AUTOCLOSE_WEAK=1` when only one WINWORD process exists.

#[cfg(not(windows))]
pub fn spawn_ccc_cr_poll() {}

#[cfg(not(windows))]
pub fn notify_ccc_temp_pdf_claimed(_pdf_path: &std::path::Path) {}

#[cfg(not(windows))]
pub fn manual_close_armed_word() -> Result<String, String> {
    Err("CCC change-request Word autoclose is Windows-only.".to_string())
}

#[cfg(windows)]
mod imp {
    use crate::services::office_intercept::extract_path_from_word_title;
    use crate::config::print_config;
    use active_win_pos_rs::get_active_window;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use sysinfo::System;
    use windows_sys::Win32::Foundation::{BOOL, FALSE, HWND, LPARAM, TRUE};
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowThreadProcessId,
        IsWindow, IsWindowVisible, PostMessageW, SetForegroundWindow, GW_OWNER, WM_CLOSE,
    };

    const CR_FLOW_TTL: Duration = Duration::from_secs(240);
    const WORD_TRACK_TTL: Duration = Duration::from_secs(180);
    const CLOSE_COOLDOWN: Duration = Duration::from_secs(12);
    const POLL_MS: u64 = 550;

    struct State {
        /// Last time foreground looked like CCC Change Requests.
        cr_flow_at: Option<Instant>,
        ccc_hwnd: Option<isize>,
        /// Word window we may close after a matching PDF.
        word: Option<WordTrack>,
        last_autoclose_at: Option<Instant>,
    }

    struct WordTrack {
        pid: u32,
        hwnd: isize,
        doc_path_lower: Option<String>,
        marked_at: Instant,
    }

    impl Default for State {
        fn default() -> Self {
            Self {
                cr_flow_at: None,
                ccc_hwnd: None,
                word: None,
                last_autoclose_at: None,
            }
        }
    }

    static STATE: Mutex<State> = Mutex::new(State {
        cr_flow_at: None,
        ccc_hwnd: None,
        word: None,
        last_autoclose_at: None,
    });

    pub fn ccc_cr_autoclose_enabled() -> bool {
        if !print_config::ccc_temp_watch_only() {
            return false;
        }
        match std::env::var("UCE_CCC_CR_AUTOCLOSE_WORD") {
            Ok(v) => {
                let v = v.trim();
                !(v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off"))
            }
            Err(_) => true,
        }
    }

    fn weak_mode_enabled() -> bool {
        std::env::var("UCE_CCC_CR_AUTOCLOSE_WEAK")
            .map(|v| v.trim() == "1")
            .unwrap_or(false)
    }

    fn title_starts_with_4_to_6_digit_ro(t: &str) -> bool {
        let t = t.trim_start();
        let mut n = 0usize;
        for ch in t.chars() {
            if ch.is_ascii_digit() {
                n += 1;
            } else {
                break;
            }
        }
        (4..=6).contains(&n)
    }

    fn is_ccc_surface(app: &str, title: &str) -> bool {
        let a = app.to_lowercase();
        let t = title.to_lowercase();
        a.contains("ccc")
            || t.contains("ccc")
            || t.contains("ccc one")
            || t.contains("cccone")
            || title_starts_with_4_to_6_digit_ro(&t)
            || t.contains("repair order")
            || t.contains("ro ")
            || t.contains("ro#")
    }

    fn title_is_change_request(title: &str) -> bool {
        let t = title.to_lowercase();
        t.contains("change request")
            || t.contains("change requests")
            || t.contains("changerequest")
            || t.contains("change-request")
    }

    fn path_under_ccc_temp(p: &Path) -> bool {
        let ccc = print_config::ccc_temp_watch_path();
        let pl = p.to_string_lossy().to_lowercase();
        let cl = ccc.to_string_lossy().to_lowercase();
        let cl = cl.trim_end_matches('\\');
        pl.starts_with(cl)
    }

    fn count_winword_processes() -> usize {
        let mut sys = System::new();
        sys.refresh_all();
        sys.processes()
            .iter()
            .filter(|(_, p)| {
                p.name()
                    .to_string_lossy()
                    .to_lowercase()
                    .contains("winword")
            })
            .count()
    }

    struct EnumCtx {
        target_pid: u32,
        best: Option<isize>,
    }

    unsafe extern "system" fn enum_top_level(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam as *mut EnumCtx);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != ctx.target_pid {
            return TRUE;
        }
        if IsWindowVisible(hwnd) == 0 {
            return TRUE;
        }
        if GetWindow(hwnd, GW_OWNER) != std::ptr::null_mut() {
            return TRUE;
        }
        let mut buf = [0u16; 256];
        let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if n <= 0 {
            return TRUE;
        }
        let cls = String::from_utf16_lossy(&buf[..n as usize]);
        let cls_lower = cls.to_lowercase();
        if cls_lower.contains("opus") {
            ctx.best = Some(hwnd as isize);
            return FALSE;
        }
        TRUE
    }

    unsafe extern "system" fn enum_fallback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam as *mut EnumCtx);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != ctx.target_pid {
            return TRUE;
        }
        if IsWindowVisible(hwnd) == 0 {
            return TRUE;
        }
        if GetWindow(hwnd, GW_OWNER) != std::ptr::null_mut() {
            return TRUE;
        }
        ctx.best = Some(hwnd as isize);
        FALSE
    }

    fn hwnd_for_pid_winword(pid: u32) -> Option<isize> {
        let mut ctx = EnumCtx {
            target_pid: pid,
            best: None,
        };
        unsafe {
            EnumWindows(
                Some(enum_top_level),
                &mut ctx as *mut EnumCtx as LPARAM,
            );
        }
        if ctx.best.is_some() {
            return ctx.best;
        }
        let mut ctx2 = EnumCtx {
            target_pid: pid,
            best: None,
        };
        unsafe {
            EnumWindows(
                Some(enum_fallback),
                &mut ctx2 as *mut EnumCtx as LPARAM,
            );
        }
        ctx2.best
    }

    fn hwnd_still_valid(hwnd: isize) -> bool {
        if hwnd == 0 {
            return false;
        }
        unsafe { IsWindow(hwnd as HWND) != 0 }
    }

    fn try_set_foreground(hwnd: isize) -> bool {
        if hwnd == 0 {
            return false;
        }
        unsafe {
            let target = hwnd as HWND;
            if IsWindow(target) == 0 {
                return false;
            }
            let fg = GetForegroundWindow();
            if fg == std::ptr::null_mut() {
                return SetForegroundWindow(target) != 0;
            }
            let mut fg_tid = 0u32;
            GetWindowThreadProcessId(fg, &mut fg_tid);
            let cur = GetCurrentThreadId();
            if AttachThreadInput(cur, fg_tid, 1) != 0 {
                let _ = SetForegroundWindow(target);
                let _ = AttachThreadInput(cur, fg_tid, 0);
                return GetForegroundWindow() == target;
            }
            SetForegroundWindow(target) != 0
        }
    }

    fn correlate_strong(wt: &WordTrack, pdf_path: &Path) -> bool {
        let Some(ref doc_s) = wt.doc_path_lower else {
            return false;
        };
        let doc = PathBuf::from(doc_s);
        if !doc.is_file() {
            // Path may have moved; still compare stem/parent strings
        }
        let doc_stem = doc
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase());
        let pdf_stem = pdf_path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase());
        if doc_stem.is_none() || pdf_stem.is_none() || doc_stem != pdf_stem {
            return false;
        }
        let pdf_parent = pdf_path.parent().map(|p| p.to_string_lossy().to_lowercase());
        let doc_parent = doc.parent().map(|p| p.to_string_lossy().to_lowercase());
        pdf_parent == doc_parent
    }

    fn correlate_weak(wt: &WordTrack, pdf_path: &Path) -> bool {
        if !weak_mode_enabled() {
            return false;
        }
        if count_winword_processes() > 1 {
            eprintln!(
                "[UCE][ccc-cr] weak_correlation_skipped reason=multiple_winword_processes"
            );
            return false;
        }
        if !path_under_ccc_temp(pdf_path) {
            return false;
        }
        if Instant::now().duration_since(wt.marked_at) > WORD_TRACK_TTL {
            return false;
        }
        wt.hwnd != 0
    }

    fn log_focus_restore(ok: bool, hwnd: isize) {
        if ok {
            eprintln!(
                "[UCE][ccc-cr] focus_restore result=success target_hwnd={}",
                hwnd
            );
        } else {
            eprintln!(
                "[UCE][ccc-cr] focus_restore result=failure target_hwnd={}",
                hwnd
            );
        }
    }

    pub fn spawn_ccc_cr_poll() {
        if !ccc_cr_autoclose_enabled() {
            eprintln!("[UCE][ccc-cr] poll_disabled reason=config_or_env");
            return;
        }
        std::thread::spawn(|| {
            eprintln!("[UCE][ccc-cr] poll_started interval_ms={}", POLL_MS);
            loop {
                std::thread::sleep(Duration::from_millis(POLL_MS));
                let _ = tick_active_window();
            }
        });
    }

    fn tick_active_window() -> Result<(), ()> {
        let w = get_active_window().map_err(|_| ())?;
        let exe = w
            .process_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        let title = w.title.trim().to_string();
        let app = w.app_name.trim().to_string();
        let pid_u = w.process_id;
        let pid = u32::try_from(pid_u).unwrap_or(0);
        let now = Instant::now();

        let mut st = STATE.lock().map_err(|_| ())?;

        // Drop stale flow
        if let Some(t) = st.cr_flow_at {
            if now.duration_since(t) > CR_FLOW_TTL {
                st.cr_flow_at = None;
                st.ccc_hwnd = None;
                st.word = None;
            }
        }
        if let Some(ref wt) = st.word {
            if now.duration_since(wt.marked_at) > WORD_TRACK_TTL {
                st.word = None;
            }
        }

        if exe.contains("winword") {
            let hwnd = hwnd_for_pid_winword(pid).unwrap_or(0);
            if let Some(cr_at) = st.cr_flow_at {
                if now.duration_since(cr_at) <= CR_FLOW_TTL {
                    let doc_path_lower = extract_path_from_word_title(&title)
                        .map(|p| p.to_string_lossy().to_lowercase());
                    eprintln!(
                        "[UCE][ccc-cr] word_window_detected pid={} hwnd={} title_sample={} path_in_title={}",
                        pid,
                        hwnd,
                        title.chars().take(80).collect::<String>(),
                        doc_path_lower.is_some()
                    );
                    st.word = Some(WordTrack {
                        pid,
                        hwnd,
                        doc_path_lower,
                        marked_at: now,
                    });
                }
            }
        } else if is_ccc_surface(&app, &title) && title_is_change_request(&title) {
            let hwnd = hwnd_for_pid_winword(pid).unwrap_or(0);
            st.cr_flow_at = Some(now);
            st.ccc_hwnd = Some(hwnd);
            eprintln!(
                "[UCE][ccc-cr] change_request_flow_detected app_sample={} title_sample={} ccc_hwnd={}",
                app.chars().take(40).collect::<String>(),
                title.chars().take(100).collect::<String>(),
                hwnd
            );
        }

        Ok(())
    }

    /// Call after a PDF was successfully claimed from CCC temp (before the original path may be moved).
    pub fn notify_ccc_temp_pdf_claimed(pdf_path: &Path) {
        if !ccc_cr_autoclose_enabled() {
            return;
        }
        if !path_under_ccc_temp(pdf_path) {
            return;
        }

        let now = Instant::now();
        let mut st = match STATE.lock() {
            Ok(g) => g,
            Err(_) => return,
        };

        if let Some(lc) = st.last_autoclose_at {
            if now.duration_since(lc) < CLOSE_COOLDOWN {
                eprintln!(
                    "[UCE][ccc-cr] auto_close_skipped reason=cooldown_active secs={}",
                    CLOSE_COOLDOWN.as_secs()
                );
                return;
            }
        }

        let Some(wt) = st.word.take() else {
            eprintln!("[UCE][ccc-cr] pdf_detected_in_temp path={} stem=? — auto_close_skipped reason=no_tracked_word_window", pdf_path.display());
            return;
        };

        eprintln!(
            "[UCE][ccc-cr] pdf_detected_in_temp path={} awaiting_correlation",
            pdf_path.display()
        );

        let strong = correlate_strong(&wt, pdf_path);
        let weak = if strong {
            false
        } else {
            correlate_weak(&wt, pdf_path)
        };

        if !strong && !weak {
            eprintln!(
                "[UCE][ccc-cr] auto_close_skipped reason=correlation_failed strong_path_required_or_set_UCE_CCC_CR_AUTOCLOSE_WEAK=1"
            );
            if now.duration_since(wt.marked_at) < WORD_TRACK_TTL {
                st.word = Some(wt);
            }
            return;
        }

        if weak && !strong {
            eprintln!("[UCE][ccc-cr] correlation mode=weak single_winword");
        } else {
            eprintln!("[UCE][ccc-cr] correlation mode=strong path_and_stem");
        }

        if !hwnd_still_valid(wt.hwnd) {
            eprintln!(
                "[UCE][ccc-cr] auto_close_skipped reason=hwnd_invalid pid={}",
                wt.pid
            );
            if now.duration_since(wt.marked_at) < WORD_TRACK_TTL {
                st.word = Some(wt);
            }
            return;
        }

        eprintln!(
            "[UCE][ccc-cr] auto_close_attempt hwnd={} pid={}",
            wt.hwnd, wt.pid
        );
        let posted = unsafe { PostMessageW(wt.hwnd as HWND, WM_CLOSE, 0, 0) };
        if posted == 0 {
            eprintln!("[UCE][ccc-cr] auto_close result=failure PostMessageW returned 0");
            if now.duration_since(wt.marked_at) < WORD_TRACK_TTL {
                st.word = Some(wt);
            }
            return;
        }
        eprintln!("[UCE][ccc-cr] auto_close result=success WM_CLOSE posted");
        st.last_autoclose_at = Some(now);

        let ccc_hwnd = st.ccc_hwnd.unwrap_or(0);
        drop(st);
        std::thread::sleep(Duration::from_millis(90));
        if ccc_hwnd != 0 && hwnd_still_valid(ccc_hwnd) {
            let ok = try_set_foreground(ccc_hwnd);
            log_focus_restore(ok, ccc_hwnd);
        } else {
            eprintln!(
                "[UCE][ccc-cr] focus_restore skipped reason=no_valid_stored_ccc_hwnd"
            );
        }
    }

    /// Hotkey / command: foreground must be Word; must be in a recent Change Request flow or tracked window.
    pub fn manual_close_armed_word() -> Result<String, String> {
        if !ccc_cr_autoclose_enabled() {
            return Err("CCC CR Word autoclose disabled.".to_string());
        }
        let w = get_active_window().map_err(|_| {
            "Could not read foreground window (active-win-pos-rs).".to_string()
        })?;
        let exe = w
            .process_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !exe.contains("winword") {
            return Err("Foreground window is not Word.".to_string());
        }
        let pid = u32::try_from(w.process_id).map_err(|_| "invalid pid".to_string())?;
        let hwnd = hwnd_for_pid_winword(pid).ok_or("Could not resolve Word window handle.")?;

        let mut st = STATE.lock().map_err(|_| "state lock poisoned".to_string())?;
        let in_flow = st
            .cr_flow_at
            .map(|t| Instant::now().duration_since(t) < CR_FLOW_TTL)
            .unwrap_or(false);
        let tracked_same = st
            .word
            .as_ref()
            .map(|wt| wt.hwnd == hwnd || wt.pid == pid)
            .unwrap_or(false);

        if !in_flow && !tracked_same {
            return Err(
                "No recent Change Request flow — open Change Requests in CCC first.".to_string(),
            );
        }

        eprintln!(
            "[UCE][ccc-cr] hotkey_close_attempt hwnd={} pid={}",
            hwnd, pid
        );
        let posted = unsafe { PostMessageW(hwnd as HWND, WM_CLOSE, 0, 0) };
        if posted == 0 {
            return Err("PostMessage WM_CLOSE failed.".to_string());
        }
        st.last_autoclose_at = Some(Instant::now());
        st.word = None;

        let ccc_hwnd = st.ccc_hwnd.unwrap_or(0);
        drop(st);
        std::thread::sleep(Duration::from_millis(90));
        if ccc_hwnd != 0 && hwnd_still_valid(ccc_hwnd) {
            let ok = try_set_foreground(ccc_hwnd);
            log_focus_restore(ok, ccc_hwnd);
        }
        Ok("Word close requested (WM_CLOSE).".to_string())
    }
}

#[cfg(windows)]
pub use imp::{manual_close_armed_word, notify_ccc_temp_pdf_claimed, spawn_ccc_cr_poll};
