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

        // zip 9：name() 解码可失败（非 UTF-8/未知编码条目名）——诚实报错拒绝
        // 整个解压（安全敏感提取器，畸形名字不允许静默转写）。
        let entry_name = entry
            .name()
            .map_err(|e| format!("failed to decode zip entry {} name: {}", idx, e))?
            .to_string();
        let entry_path = dest_dir.join(&entry_name);

        // Security check: ensure the resolved path is within the destination directory.
        // We canonicalize the parent (which must exist since we created dest_dir) and
        // then check if it's within the destination.
        if let Some(parent) = entry_path.parent()
            && let Ok(parent_canonical) = parent.canonicalize()
            && !parent_canonical.starts_with(&dest_dir_abs)
        {
            return Err(format!(
                "invalid file path: {} (zip slip detected)",
                entry_name
            ));
        }

        // 词法闸（单一真相源 [`is_unsafe_rel_path`]）：不依赖文件系统状态。
        // 上方 canonicalize 型检查在父目录不存在时被静默跳过，而新装目录的
        // 常态恰是父目录缺席——`..`/盘符穿越必须由词法闸兜死。
        if is_unsafe_rel_path(&entry_name) {
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

/// 词法级相对路径安全闸（zip-slip / 归档穿越判定的单一真相源）。
///
/// canonicalize 型检查（父目录规范化后 `starts_with` 目标目录）在**父目录
/// 尚不存在**时会被静默跳过——而新装目录的常态恰是父目录缺席，穿越路径
/// 只要经过一个不存在的中间目录即可绕过（`newdir/../../../evil`）。词法
/// 闸不依赖文件系统状态：拒绝绝对路径（含 Windows 盘符前缀——`C:/x` 在
/// Unix 解析为普通组件，按字节显式识别）与一切非 Normal 组件（`..`、根
/// 分隔符、`.` 自引用）。合法名字如 `a..b.txt` 不误伤（按组件而非子串）。
///
/// 判定前先做反斜杠归一：`\` 在 Unix 是合法文件名字符，Path 组件解析不切
/// 分，`foo\..\..\bar.txt` 会整体落成单个 Normal 组件而漏放（Linux CI 实
/// 证）。归一只会更严不更松，Windows/POSIX 形态同判。
pub fn is_unsafe_rel_path(rel: &str) -> bool {
    let normalized = rel.replace('\\', "/");
    let p = Path::new(&normalized);
    if p.is_absolute() {
        return true;
    }
    let b = normalized.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return true; // Windows 盘符前缀（"C:/x"、"C:\x"），跨平台一致拒绝
    }
    p.components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
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

    // finish returns the inner BufWriter — dropping it as a temporary would
    // silently swallow the final flush error (≤8KiB tail), reporting success
    // while the zip may be truncated. Flush explicitly so errors surface.
    let inner = zip_writer
        .finish()
        .map_err(|e| format!("failed to finalize zip archive: {}", e))?;
    inner
        .into_inner()
        .map_err(|e| format!("failed to flush zip archive: {}", e.into_error()))?;

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
                .add_directory(format!("{}/", relative_str), *options)
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
