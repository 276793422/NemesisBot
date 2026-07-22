//! File utilities.

use std::fs;
use std::io::Write;
use std::path::Path;

/// Atomically write data to a file using temp file + rename.
pub fn write_file_atomic(path: &str, data: &[u8], _perm: u32) -> Result<(), String> {
    let p = Path::new(path);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
    }

    let tmp_name = format!(
        ".tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let tmp_path = p.parent().unwrap_or(Path::new(".")).join(tmp_name);

    let mut tmp_file = fs::File::create(&tmp_path).map_err(|e| format!("create temp: {}", e))?;

    tmp_file
        .write_all(data)
        .map_err(|e| format!("write temp: {}", e))?;
    tmp_file.flush().map_err(|e| format!("flush: {}", e))?;
    drop(tmp_file);

    fs::rename(&tmp_path, path).map_err(|e| format!("rename: {}", e))?;

    Ok(())
}

/// Ensure a directory exists.
pub fn ensure_dir(path: &str) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("create dir: {}", e))
}

/// Read file to string.
pub fn read_file_string(path: &str) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("read: {}", e))
}

#[cfg(test)]
mod tests;
