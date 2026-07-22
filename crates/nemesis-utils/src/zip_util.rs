//! Zip extraction and creation utilities.
//!
//! Provides safe ZIP file extraction with zip-slip protection and directory
//! creation, as well as recursive directory-to-ZIP creation.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use zip::ZipArchive;

/// Extract a ZIP file to the specified directory.
///
/// Creates all necessary directories. Validates each entry's destination path
/// to prevent zip-slip (path traversal) attacks.
///
/// # Errors
/// Returns an error string describing what went wrong.
pub fn extract_zip(zip_path: &str, dest_dir: &str) -> Result<(), String> {
    let zip_path = Path::new(zip_path);
    let dest_dir = Path::new(dest_dir);

    let file = File::open(zip_path)
        .map_err(|e| format!("failed to open zip file {}: {}", zip_path.display(), e))?;

    let reader = BufReader::new(file);
    let mut archive =
        ZipArchive::new(reader).map_err(|e| format!("failed to read zip archive: {}", e))?;

    // Ensure destination directory exists
    fs::create_dir_all(dest_dir)
        .map_err(|e| format!("failed to create destination directory: {}", e))?;

    let dest_dir_abs = dest_dir
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize dest dir: {}", e))?;

    for idx in 0..archive.len() {
        let mut entry = archive
            .by_index(idx)
            .map_err(|e| format!("failed to read zip entry {}: {}", idx, e))?;

        let entry_name = entry.name().to_string();
        let entry_path = dest_dir.join(&entry_name);

        // Security check: ensure the resolved path is within the destination directory.
        // We canonicalize the parent (which must exist since we created dest_dir) and
        // then check if it's within the destination.
        if let Some(parent) = entry_path.parent() {
            if let Ok(parent_canonical) = parent.canonicalize() {
                if !parent_canonical.starts_with(&dest_dir_abs) {
                    return Err(format!(
                        "invalid file path: {} (zip slip detected)",
                        entry_name
                    ));
                }
            }
        }

        // Additional check: reject entries with path traversal components
        let entry_name_lower = entry_name.to_lowercase();
        if entry_name_lower.starts_with("..")
            || entry_name_lower.contains("/..")
            || entry_name_lower.contains("\\..")
        {
            return Err(format!(
                "invalid file path: {} (zip slip detected)",
                entry_name
            ));
        }

        if entry.is_dir() {
            fs::create_dir_all(&entry_path).map_err(|e| {
                format!("failed to create directory {}: {}", entry_path.display(), e)
            })?;
        } else {
            // Create parent directory if needed
            if let Some(parent) = entry_path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!(
                        "failed to create parent directory {}: {}",
                        parent.display(),
                        e
                    )
                })?;
            }

            let mut out_file = File::create(&entry_path)
                .map_err(|e| format!("failed to create file {}: {}", entry_path.display(), e))?;

            let mut buf = [0u8; 8192];
            loop {
                let n = entry
                    .read(&mut buf)
                    .map_err(|e| format!("failed to read zip entry {}: {}", entry_name, e))?;
                if n == 0 {
                    break;
                }
                out_file
                    .write_all(&buf[..n])
                    .map_err(|e| format!("failed to write file {}: {}", entry_path.display(), e))?;
            }
        }
    }

    Ok(())
}

/// Create a ZIP file from all files in a source directory.
///
/// Walks the source directory recursively and adds every file to the ZIP
/// archive, preserving the relative directory structure.
///
/// # Errors
/// Returns an error string describing what went wrong.
pub fn create_zip(source_dir: &str, zip_path: &str) -> Result<(), String> {
    let source_dir = Path::new(source_dir);
    let zip_path = Path::new(zip_path);

    if !source_dir.is_dir() {
        return Err(format!(
            "source directory does not exist: {}",
            source_dir.display()
        ));
    }

    // Ensure parent directory of the zip file exists
    if let Some(parent) = zip_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create parent directory: {}", e))?;
    }

    let file = File::create(zip_path)
        .map_err(|e| format!("failed to create zip file {}: {}", zip_path.display(), e))?;

    let writer = BufWriter::new(file);
    let mut zip_writer = zip::ZipWriter::new(writer);

    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    recursively_add_directory(&mut zip_writer, source_dir, source_dir, &options)?;

    zip_writer
        .finish()
        .map_err(|e| format!("failed to finalize zip archive: {}", e))?;

    Ok(())
}

/// Recursively add all files in `current_dir` to the zip writer, using
/// `base_dir` to compute the relative path stored inside the archive.
fn recursively_add_directory<W: Write + std::io::Seek>(
    zip_writer: &mut zip::ZipWriter<W>,
    base_dir: &Path,
    current_dir: &Path,
    options: &zip::write::SimpleFileOptions,
) -> Result<(), String> {
    let entries = fs::read_dir(current_dir)
        .map_err(|e| format!("failed to read directory {}: {}", current_dir.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read dir entry: {}", e))?;
        let path = entry.path();

        let relative = path
            .strip_prefix(base_dir)
            .map_err(|e| format!("failed to compute relative path: {}", e))?;

        // Use forward slashes for ZIP compatibility
        let relative_str = relative.to_string_lossy().replace('\\', "/");

        if path.is_dir() {
            // Add directory entry
            zip_writer
                .add_directory(&format!("{}/", relative_str), *options)
                .map_err(|e| format!("failed to add directory {} to zip: {}", relative_str, e))?;

            // Recurse
            recursively_add_directory(zip_writer, base_dir, &path, options)?;
        } else {
            // Add file entry
            zip_writer
                .start_file(&relative_str, *options)
                .map_err(|e| format!("failed to start file {} in zip: {}", relative_str, e))?;

            let mut file = File::open(&path)
                .map_err(|e| format!("failed to open file {}: {}", path.display(), e))?;

            let mut buf = [0u8; 8192];
            loop {
                let n = file
                    .read(&mut buf)
                    .map_err(|e| format!("failed to read file {}: {}", path.display(), e))?;
                if n == 0 {
                    break;
                }
                zip_writer
                    .write_all(&buf[..n])
                    .map_err(|e| format!("failed to write file data to zip: {}", e))?;
            }
        }
    }

    Ok(())
}

/// Check whether `path` is within `base_dir` (zip-slip prevention).
///
/// If both paths exist, they are canonicalized for reliable comparison.
/// If either path does not exist, a string-based prefix check is used instead.
pub fn is_path_within_dir(path: &Path, base_dir: &Path) -> bool {
    // Try canonicalize first (most reliable)
    let abs_path = path.canonicalize().ok();
    let abs_base = base_dir.canonicalize().ok();

    match (abs_path, abs_base) {
        (Some(ap), Some(ab)) => {
            // Both exist: use proper prefix check
            ap.starts_with(&ab)
        }
        _ => {
            // At least one doesn't exist yet; use string-based comparison.
            // Normalize both to absolute paths for comparison.
            let path_abs = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir().unwrap_or_default().join(path)
            };
            let base_abs = if base_dir.is_absolute() {
                base_dir.to_path_buf()
            } else {
                std::env::current_dir().unwrap_or_default().join(base_dir)
            };

            let path_str = normalize_path_str(&path_abs);
            let base_str = normalize_path_str(&base_abs);

            if path_str.starts_with(&base_str) {
                // Make sure it's not a partial match (e.g. /tmp/base-other matching /tmp/base)
                let rest = &path_str[base_str.len()..];
                rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\')
            } else {
                false
            }
        }
    }
}

/// Normalize a path to a string with consistent separators.
fn normalize_path_str(path: &Path) -> String {
    let mut s = path.to_string_lossy().to_string();
    // Ensure trailing separator for directory comparison
    s = s.replace('\\', "/");
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

#[cfg(test)]
mod tests;
