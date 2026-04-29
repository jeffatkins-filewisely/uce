//! Persistent log for native popup attempts (`C:\\FileWisely\\logs\\popup.log` on Windows).
//! Survives missing console / GUI-only launches.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

/// Fixed path per product request (Windows). Non-Windows: temp dir fallback.
pub fn log_path() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(r"C:\FileWisely\logs\popup.log")
    }
    #[cfg(not(windows))]
    {
        std::env::temp_dir().join("uce-popup.log")
    }
}

/// Appends one line: `timestamp | kind | source | message` (message is single-line; `|` in text escaped).
pub fn append(kind: &str, source: &str, message: &str) {
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
    let msg = sanitize_for_log(message);
    let line = format!("{} | {} | {} | {}\n", ts, kind, source, msg);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    }
}

fn sanitize_for_log(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\r' | '\n' => ' ',
            '|' => '¦',
            _ => c,
        })
        .collect::<String>()
}

/// `(path display string, last n non-empty lines)` for Connection Doctor.
pub fn read_last_n_lines(n: usize) -> (String, Vec<String>) {
    let path = log_path();
    let path_str = path.to_string_lossy().into_owned();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return (path_str, Vec::new());
    };
    let mut lines: Vec<String> = raw
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    if lines.len() > n {
        lines = lines.split_off(lines.len().saturating_sub(n));
    }
    (path_str, lines)
}
