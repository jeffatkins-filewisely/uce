//! Copy or consolidate drops into the standard FileWisely Incoming folder (e.g. after “Save as” elsewhere).

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::print_config::FW_OUTPUT_DIR;

/// Copy `input_path` into [`FW_OUTPUT_DIR`] using the same file name (overwrite if present).
pub fn normalize_into_incoming(input_path: &Path) -> Result<PathBuf, String> {
    if !input_path.is_file() {
        return Err("File not found".into());
    }
    let dest_dir = Path::new(FW_OUTPUT_DIR);
    fs::create_dir_all(dest_dir).map_err(|e| format!("create incoming dir: {e}"))?;
    let name = input_path
        .file_name()
        .ok_or_else(|| "Invalid file name".to_string())?;
    let new_path = dest_dir.join(name);
    fs::copy(input_path, &new_path).map_err(|e| e.to_string())?;
    Ok(new_path)
}
