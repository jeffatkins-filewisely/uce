//! Re-apply Bullzip silent INI (no Save As, no open-after-print) on UCE startup and after printer repair.
//! Writes per-user `%APPDATA%` always; ProgramData when the process can create those paths.

use crate::config::print_config::FW_OUTPUT_DIR;
use std::fs;
use std::path::PathBuf;

const INI_BODY_TEMPLATE: &str = r"[PDF Printer]
Output={output}
ShowSaveAS=never
ShowSettings=never
ShowPDF=no
ConfirmOverwrite=no
DisableOptionDialog=yes
OpenFolder=no
OpenPDF=no
";

fn incoming_output_line() -> String {
    let inc = FW_OUTPUT_DIR.trim_end_matches(['\\', '/']);
    format!(r"{inc}\<date>_<time>_<docname>.pdf")
}

fn ini_body() -> String {
    INI_BODY_TEMPLATE.replace("{output}", &incoming_output_line())
}

fn write_ini(path: &PathBuf) -> bool {
    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    match fs::write(path, ini_body()) {
        Ok(()) => {
            eprintln!("[UCE] bullzip_silent_ini written path={}", path.display());
            true
        }
        Err(e) => {
            eprintln!(
                "[UCE] bullzip_silent_ini skip path={} err={}",
                path.display(),
                e
            );
            false
        }
    }
}

#[cfg(windows)]
fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(pd) = std::env::var("ProgramData") {
        let pdf_writer = PathBuf::from(&pd).join("PDF Writer");
        paths.push(pdf_writer.join("FileWisely Printer").join("global.ini"));
        paths.push(pdf_writer.join("Bullzip PDF Printer").join("global.ini"));
        paths.push(
            PathBuf::from(&pd)
                .join("Bullzip")
                .join("PDF Printer")
                .join("global.ini"),
        );
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        paths.push(
            PathBuf::from(&appdata)
                .join("Bullzip")
                .join("PDF Printer")
                .join("settings.ini"),
        );
    }
    paths
}

/// Best-effort silent INI refresh (no elevation required for APPDATA).
#[cfg(windows)]
pub fn ensure_silent_bullzip_ini() {
    let mut wrote = 0usize;
    for p in candidate_paths() {
        if write_ini(&p) {
            wrote += 1;
        }
    }
    eprintln!("[UCE] bullzip_silent_ini paths_written={}", wrote);
}

#[cfg(not(windows))]
pub fn ensure_silent_bullzip_ini() {}
