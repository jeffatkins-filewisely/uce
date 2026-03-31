//! Give each file in `C:\FileWisely\Incoming` a unique `fw_<ts>_<hex>.<ext>` name so repeated
//! `document.pdf` / `document.docx` saves do not overwrite. First rename attempt is immediate (no sleep).

use crate::config::print_config;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn is_direct_child_of_filewisely_incoming(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let incoming = PathBuf::from(print_config::FW_OUTPUT_DIR);
    let Some(parent) = path.parent() else {
        return false;
    };
    match (fs::canonicalize(parent), fs::canonicalize(&incoming)) {
        (Ok(p), Ok(i)) => p == i,
        _ => parent
            .to_string_lossy()
            .eq_ignore_ascii_case(&incoming.to_string_lossy()),
    }
}

/// True if `path` is a `.pdf` directly under FileWisely Incoming.
fn is_direct_filewisely_incoming_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
        && is_direct_child_of_filewisely_incoming(path)
}

/// True if `path` is `.doc` / `.docx` / `.rtf` directly under FileWisely Incoming.
fn is_direct_filewisely_incoming_office(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());
    let Some(ext) = ext else {
        return false;
    };
    matches!(ext.as_str(), "doc" | "docx" | "rtf") && is_direct_child_of_filewisely_incoming(path)
}

/// Stem is `fw_<digits>_<6 hex>` (before extension).
fn is_fw_unique_stem(stem: &str) -> bool {
    let mut parts = stem.split('_');
    let Some("fw") = parts.next() else {
        return false;
    };
    let Some(ts) = parts.next() else {
        return false;
    };
    let Some(hexpart) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    ts.chars().all(|c| c.is_ascii_digit())
        && hexpart.len() == 6
        && hexpart.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_fw_unique_pdf_name(name: &str) -> bool {
    let l = name.to_lowercase();
    if !l.ends_with(".pdf") {
        return false;
    }
    let stem = &l[..l.len() - 4];
    is_fw_unique_stem(stem)
}

fn is_fw_unique_office_name(name: &str) -> bool {
    let l = name.to_lowercase();
    let ext_len = if l.ends_with(".docx") {
        5
    } else if l.ends_with(".doc") {
        4
    } else if l.ends_with(".rtf") {
        4
    } else {
        return false;
    };
    let stem = &l[..l.len() - ext_len];
    is_fw_unique_stem(stem)
}

fn rename_to_fw_unique(path: PathBuf, ext_for_new_file: &str) -> PathBuf {
    let orig_display = path.display().to_string();
    let Some(parent) = path.parent().map(Path::to_path_buf) else {
        return path;
    };

    for attempt in 0..48u32 {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let rnd = ((SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u128)
            .wrapping_add(attempt as u128 * 7919)
            % 0xFFFFFF) as u32;
        let new_name = format!("fw_{ts}_{:06x}{ext_for_new_file}", rnd);
        let dest = parent.join(&new_name);
        if dest.exists() {
            continue;
        }
        for retry in 0..20 {
            if retry == 0 {
                eprintln!("[UCE] Rename attempt immediate path={orig_display}");
            }
            match fs::rename(&path, &dest) {
                Ok(()) => {
                    eprintln!(
                        "[UCE][pipeline] RENAMED path={} from={}",
                        dest.display(),
                        orig_display
                    );
                    return dest;
                }
                Err(e) => {
                    if retry < 19 {
                        eprintln!(
                            "[UCE] Rename retry (locked) n={} path={} err={}",
                            retry + 1,
                            orig_display,
                            e
                        );
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        }
    }

    eprintln!(
        "[UCE] Renamed: gave up after retries, continuing with original path: {orig_display}"
    );
    path
}

/// Rename `document.pdf` → `fw_<timestamp>_<random>.pdf` immediately on detection.
pub fn unique_rename_incoming_pdf_if_needed(path: PathBuf) -> PathBuf {
    if !is_direct_filewisely_incoming_pdf(&path) {
        return path;
    }
    let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
        return path;
    };
    if is_fw_unique_pdf_name(fname) {
        return path;
    }
    rename_to_fw_unique(path, ".pdf")
}

/// Rename `document.docx` (etc.) → `fw_<timestamp>_<random>.docx` before claim/convert (same overwrite protection as PDF).
pub fn unique_rename_incoming_office_if_needed(path: PathBuf) -> PathBuf {
    if !is_direct_filewisely_incoming_office(&path) {
        return path;
    }
    let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
        return path;
    };
    if is_fw_unique_office_name(fname) {
        return path;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("docx");
    let ext_lower = format!(".{}", ext.to_lowercase());
    rename_to_fw_unique(path, &ext_lower)
}
