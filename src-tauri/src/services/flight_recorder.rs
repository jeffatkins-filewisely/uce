//! Diagnostic “flight recorder”: correlates WINWORD launches with recent filesystem activity.
//! Enable with `UCE_FLIGHT_RECORDER=1`. Append-only log: `C:\FileWisely\logs\uce-debug.log`.
//!
//! Does **not** move, convert, upload, or enqueue files—observation only.

#[cfg(windows)]
mod imp {
    use active_win_pos_rs::get_active_window;
    use notify::event::{Event as NotifyEvent, EventKind, ModifyKind};
    use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
    use serde::Serialize;
    use std::collections::{HashSet, VecDeque};
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use sysinfo::{Pid, System};

    const LOG_PATH: &str = r"C:\FileWisely\logs\uce-debug.log";
    const RING_CAP: usize = 800;
    const WORD_LOOKBACK_MS: u128 = 3_000;
    const RECENT_IN_CONTEXT: usize = 20;

    pub fn enabled() -> bool {
        std::env::var("UCE_FLIGHT_RECORDER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    fn skip_roaming_recursive() -> bool {
        std::env::var("UCE_FLIGHT_RECORDER_SKIP_ROAMING")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    fn now_unix_ms() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }

    #[derive(Clone, Serialize)]
    struct FileEventRecord {
        time: u128,
        event_type: String,
        full_path: String,
        extension: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    }

    fn extension_allowed(path: &Path) -> Option<String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())?;
        if matches!(ext.as_str(), "doc" | "docx" | "rtf" | "tmp" | "pdf") {
            Some(ext)
        } else {
            None
        }
    }

    fn event_type_str(kind: &EventKind) -> &'static str {
        match kind {
            EventKind::Create(_) => "create",
            EventKind::Modify(ModifyKind::Name(_)) => "rename",
            EventKind::Modify(_) => "change",
            EventKind::Remove(_) => "remove",
            EventKind::Other => "other",
            EventKind::Access(_) => "access",
            EventKind::Any => "any",
        }
    }

    fn file_size_if_file(path: &Path) -> Option<u64> {
        fs::metadata(path).ok().filter(|m| m.is_file()).map(|m| m.len())
    }

    fn append_log_line(json: &str) {
        let line = format!("{}\n", json);
        let _ = fs::create_dir_all(r"C:\FileWisely\logs");
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(LOG_PATH) {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
        eprintln!("[UCE][flight_recorder] {}", json);
    }

    fn log_json<K: Serialize>(kind: &str, payload: &K) {
        #[derive(Serialize)]
        struct Envelope<'a, T> {
            kind: &'a str,
            #[serde(flatten)]
            inner: T,
        }
        let env = Envelope { kind, inner: payload };
        match serde_json::to_string(&env) {
            Ok(s) => append_log_line(&s),
            Err(e) => eprintln!("[UCE][flight_recorder] serialize err: {e}"),
        }
    }

    fn push_file_event(
        ring: &Arc<Mutex<VecDeque<FileEventRecord>>>,
        time: u128,
        event_type: &str,
        path: &Path,
    ) {
        let Some(ext) = extension_allowed(path) else {
            return;
        };
        let full_path = path.to_string_lossy().to_string();
        let size = file_size_if_file(path);
        let rec = FileEventRecord {
            time,
            event_type: event_type.to_string(),
            full_path,
            extension: ext,
            size,
        };
        #[derive(Serialize)]
        struct FileEventPayload {
            time: u128,
            event_type: String,
            full_path: String,
            extension: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            size: Option<u64>,
        }
        log_json(
            "FILE_EVENT",
            &FileEventPayload {
                time: rec.time,
                event_type: rec.event_type.clone(),
                full_path: rec.full_path.clone(),
                extension: rec.extension.clone(),
                size: rec.size,
            },
        );
        if let Ok(mut g) = ring.lock() {
            g.push_back(rec);
            while g.len() > RING_CAP {
                g.pop_front();
            }
        }
    }

    fn collect_recent_for_word_launch(
        ring: &Arc<Mutex<VecDeque<FileEventRecord>>>,
        launch_ms: u128,
    ) -> Vec<FileEventRecord> {
        let cutoff = launch_ms.saturating_sub(WORD_LOOKBACK_MS);
        let Ok(g) = ring.lock() else {
            return Vec::new();
        };
        g.iter()
            .filter(|e| e.time >= cutoff && e.time <= launch_ms)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .take(RECENT_IN_CONTEXT)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    fn window_title_for_pid(target: u64) -> String {
        thread::sleep(Duration::from_millis(600));
        match get_active_window() {
            Ok(w) if w.process_id == target => w.title.trim().to_string(),
            Ok(w) => format!("(active_other pid={} title=\"{}\")", w.process_id, w.title),
            Err(_) => String::new(),
        }
    }

    fn parent_info(sys: &System, parent: Option<Pid>) -> String {
        let Some(ppid) = parent else {
            return String::new();
        };
        let Some(p) = sys.process(ppid) else {
            return format!("pid={}", ppid.as_u32());
        };
        let name = p.name().to_string_lossy();
        let exe = p
            .exe()
            .map(|x| x.display().to_string())
            .unwrap_or_default();
        format!("pid={} name={} exe={}", ppid.as_u32(), name, exe)
    }

    fn spawn_file_watchers(ring: Arc<Mutex<VecDeque<FileEventRecord>>>) {
        thread::spawn(move || {
            let mut paths: Vec<PathBuf> = Vec::new();
            if let Ok(local) = std::env::var("LOCALAPPDATA") {
                let temp = PathBuf::from(&local).join("Temp");
                paths.push(temp);
            }
            if let Ok(profile) = std::env::var("USERPROFILE") {
                let base = PathBuf::from(&profile);
                paths.push(base.join("Downloads"));
                paths.push(base.join("Documents"));
            }
            if let Ok(roam) = std::env::var("APPDATA") {
                paths.push(PathBuf::from(roam));
            }

            let ring_cb = ring.clone();
            let mut watcher = match RecommendedWatcher::new(
                move |res: Result<NotifyEvent, notify::Error>| {
                    let t = now_unix_ms();
                    match res {
                        Ok(ev) => {
                            let et = event_type_str(&ev.kind);
                            for p in ev.paths {
                                push_file_event(&ring_cb, t, et, &p);
                            }
                        }
                        Err(e) => eprintln!("[UCE][flight_recorder] notify err: {e}"),
                    }
                },
                Config::default(),
            ) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("[UCE][flight_recorder] watcher create failed: {e}");
                    return;
                }
            };

            for p in paths {
                if !p.exists() {
                    eprintln!(
                        "[UCE][flight_recorder] skip missing watch path: {}",
                        p.display()
                    );
                    continue;
                }
                let mode = if skip_roaming_recursive()
                    && p.file_name().is_some_and(|n| {
                        n.to_string_lossy()
                            .eq_ignore_ascii_case("Roaming")
                            || p.to_string_lossy().to_lowercase().ends_with("\\appdata\\roaming")
                    }) {
                    RecursiveMode::NonRecursive
                } else {
                    RecursiveMode::Recursive
                };
                if let Err(e) = watcher.watch(&p, mode) {
                    eprintln!(
                        "[UCE][flight_recorder] watch failed {}: {e}",
                        p.display()
                    );
                } else {
                    eprintln!(
                        "[UCE][flight_recorder] watching {:?} mode={:?}",
                        p, mode
                    );
                }
            }

            loop {
                thread::sleep(Duration::from_secs(3600));
            }
        });
    }

    fn spawn_process_and_focus_monitor(ring: Arc<Mutex<VecDeque<FileEventRecord>>>) {
        thread::spawn(move || {
            let mut sys = System::new();
            let mut seen_winword: HashSet<u32> = HashSet::new();
            let mut last_focus_key: Option<(u64, String)> = None;

            loop {
                thread::sleep(Duration::from_millis(400));
                sys.refresh_all();

                if let Ok(w) = get_active_window() {
                    let exe = w
                        .process_path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if exe.contains("winword") {
                        let key = (w.process_id, w.title.clone());
                        if last_focus_key.as_ref() != Some(&key) {
                            last_focus_key = Some(key.clone());
                            #[derive(Serialize)]
                            struct FocusPayload {
                                time: u128,
                                process: &'static str,
                                title: String,
                                pid: u64,
                            }
                            log_json(
                                "WINDOW_FOCUS",
                                &FocusPayload {
                                    time: now_unix_ms(),
                                    process: "WINWORD.EXE",
                                    title: w.title,
                                    pid: w.process_id,
                                },
                            );
                        }
                    }
                }

                for (pid, process) in sys.processes().iter() {
                    let name = process.name().to_string_lossy().to_lowercase();
                    if !name.contains("winword") {
                        continue;
                    }
                    let pid_u = (*pid).as_u32();
                    if seen_winword.contains(&pid_u) {
                        continue;
                    }
                    seen_winword.insert(pid_u);

                    let launch_ms = now_unix_ms();
                    let cmd_line = process
                        .cmd()
                        .iter()
                        .map(|s| s.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(" ");
                    let exe_path = process
                        .exe()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    let parent = parent_info(&sys, process.parent());
                    let title = window_title_for_pid(pid_u as u64);

                    #[derive(Serialize)]
                    struct LaunchPayload {
                        time: u128,
                        process: &'static str,
                        command_line: String,
                        executable_path: String,
                        parent_process: String,
                        window_title: String,
                        pid: u32,
                    }
                    log_json(
                        "PROCESS_LAUNCH",
                        &LaunchPayload {
                            time: launch_ms,
                            process: "WINWORD.EXE",
                            command_line: cmd_line.clone(),
                            executable_path: exe_path,
                            parent_process: parent,
                            window_title: title.clone(),
                            pid: pid_u,
                        },
                    );

                    let recent = collect_recent_for_word_launch(&ring, launch_ms);
                    #[derive(Serialize)]
                    struct WordCtxPayload {
                        time: u128,
                        recent_files: Vec<FileEventRecord>,
                        command_line: String,
                        window_title: String,
                        pid: u32,
                    }
                    log_json(
                        "WORD_CONTEXT",
                        &WordCtxPayload {
                            time: launch_ms,
                            recent_files: recent,
                            command_line: cmd_line,
                            window_title: title,
                            pid: pid_u,
                        },
                    );
                }

                let alive: HashSet<u32> = sys
                    .processes()
                    .iter()
                    .filter(|(_, p)| {
                        p.name()
                            .to_string_lossy()
                            .to_lowercase()
                            .contains("winword")
                    })
                    .map(|(pid, _)| (*pid).as_u32())
                    .collect();
                seen_winword.retain(|p| alive.contains(p));
            }
        });
    }

    pub fn spawn() {
        if !enabled() {
            return;
        }
        eprintln!(
            "[UCE][flight_recorder] ENABLED → log={} (set UCE_FLIGHT_RECORDER=0 to disable)",
            LOG_PATH
        );
        let ring = Arc::new(Mutex::new(VecDeque::new()));
        spawn_file_watchers(ring.clone());
        spawn_process_and_focus_monitor(ring);
    }
}

#[cfg(windows)]
pub use imp::spawn;

#[cfg(not(windows))]
pub fn spawn() {}
