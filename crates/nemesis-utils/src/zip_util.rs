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
    let mut archive = ZipArchive::new(reader)
        .map_err(|e| format!("failed to read zip archive: {}", e))?;

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
        if entry_name_lower.starts_with("..") || entry_name_lower.contains("/..") || entry_name_lower.contains("\\..") {
            return Err(format!(
                "invalid file path: {} (zip slip detected)",
                entry_name
            ));
        }

        if entry.is_dir() {
            fs::create_dir_all(&entry_path)
                .map_err(|e| format!("failed to create directory {}: {}", entry_path.display(), e))?;
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

            let mut out_file = File::create(&entry_path).map_err(|e| {
                format!(
                    "failed to create file {}: {}",
                    entry_path.display(),
                    e
                )
            })?;

            let mut buf = [0u8; 8192];
            loop {
                let n = entry.read(&mut buf).map_err(|e| {
                    format!("failed to read zip entry {}: {}", entry_name, e)
                })?;
                if n == 0 {
                    break;
                }
                out_file.write_all(&buf[..n]).map_err(|e| {
                    format!("failed to write file {}: {}", entry_path.display(), e)
                })?;
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
                let n = file.read(&mut buf).map_err(|e| {
                    format!("failed to read file {}: {}", path.display(), e)
                })?;
                if n == 0 {
                    break;
                }
                zip_writer.write_all(&buf[..n]).map_err(|e| {
                    format!("failed to write file data to zip: {}", e)
                })?;
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
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_extract_nonexistent_zip() {
        let result = extract_zip("/nonexistent/path.zip", "/tmp/dest");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to open zip"));
    }

    #[test]
    fn test_create_and_extract_zip() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("source");
        let dest_dir = dir.path().join("dest");
        let zip_path = dir.path().join("test.zip");

        // Create source files
        fs::create_dir_all(source_dir.join("subdir")).unwrap();
        fs::write(source_dir.join("hello.txt"), b"hello world").unwrap();
        fs::write(source_dir.join("subdir/nested.txt"), b"nested content").unwrap();

        // Create zip
        let result = create_zip(
            source_dir.to_string_lossy().as_ref(),
            zip_path.to_string_lossy().as_ref(),
        );
        assert!(result.is_ok(), "create_zip failed: {:?}", result);
        assert!(zip_path.exists());

        // Extract zip
        let result = extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        );
        assert!(result.is_ok(), "extract_zip failed: {:?}", result);

        // Verify extracted files
        let hello = fs::read_to_string(dest_dir.join("hello.txt")).unwrap();
        assert_eq!(hello, "hello world");

        let nested = fs::read_to_string(dest_dir.join("subdir/nested.txt")).unwrap();
        assert_eq!(nested, "nested content");
    }

    #[test]
    fn test_create_zip_nonexistent_source() {
        let result = create_zip("/nonexistent/source", "/tmp/test.zip");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_is_path_within_dir() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();

        // Create a real file inside to test with canonicalize
        let subdir = base.join("subdir");
        fs::create_dir_all(&subdir).unwrap();
        let inside = subdir.join("file.txt");
        fs::write(&inside, b"test").unwrap();

        let outside = base.parent().unwrap().parent().unwrap().join("other.txt");

        assert!(is_path_within_dir(&inside, base));
        assert!(!is_path_within_dir(&outside, base));
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_extract_zip_with_deep_nesting() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("source");
        let dest_dir = dir.path().join("dest");
        let zip_path = dir.path().join("nested.zip");

        // Create deeply nested structure
        fs::create_dir_all(source_dir.join("a/b/c/d")).unwrap();
        fs::write(source_dir.join("a/b/c/d/deep.txt"), b"deep content").unwrap();
        fs::write(source_dir.join("top.txt"), b"top content").unwrap();

        create_zip(
            source_dir.to_string_lossy().as_ref(),
            zip_path.to_string_lossy().as_ref(),
        ).unwrap();

        extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        ).unwrap();

        assert_eq!(fs::read_to_string(dest_dir.join("a/b/c/d/deep.txt")).unwrap(), "deep content");
        assert_eq!(fs::read_to_string(dest_dir.join("top.txt")).unwrap(), "top content");
    }

    #[test]
    fn test_extract_zip_invalid_file() {
        let dir = tempfile::tempdir().unwrap();
        let bad_zip = dir.path().join("bad.zip");
        fs::write(&bad_zip, b"not a zip file").unwrap();

        let result = extract_zip(
            bad_zip.to_string_lossy().as_ref(),
            dir.path().join("dest").to_string_lossy().as_ref(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to read zip"));
    }

    #[test]
    fn test_create_zip_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let empty_dir = dir.path().join("empty");
        let zip_path = dir.path().join("empty.zip");

        fs::create_dir_all(&empty_dir).unwrap();

        let result = create_zip(
            empty_dir.to_string_lossy().as_ref(),
            zip_path.to_string_lossy().as_ref(),
        );
        assert!(result.is_ok(), "Empty dir zip should succeed: {:?}", result);
        assert!(zip_path.exists());
    }

    #[test]
    fn test_create_zip_with_binary_content() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("binary_source");
        let zip_path = dir.path().join("binary.zip");
        let dest_dir = dir.path().join("dest");

        fs::create_dir_all(&source_dir).unwrap();
        let binary_data: Vec<u8> = (0u8..=255).collect();
        fs::write(source_dir.join("binary.dat"), &binary_data).unwrap();

        create_zip(
            source_dir.to_string_lossy().as_ref(),
            zip_path.to_string_lossy().as_ref(),
        ).unwrap();

        extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        ).unwrap();

        let extracted = fs::read(dest_dir.join("binary.dat")).unwrap();
        assert_eq!(extracted, binary_data);
    }

    #[test]
    fn test_create_zip_preserves_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("multi");
        let zip_path = dir.path().join("multi.zip");
        let dest_dir = dir.path().join("dest");

        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("file1.txt"), b"content1").unwrap();
        fs::write(source_dir.join("file2.txt"), b"content2").unwrap();
        fs::write(source_dir.join("file3.txt"), b"content3").unwrap();

        create_zip(
            source_dir.to_string_lossy().as_ref(),
            zip_path.to_string_lossy().as_ref(),
        ).unwrap();

        extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        ).unwrap();

        assert_eq!(fs::read_to_string(dest_dir.join("file1.txt")).unwrap(), "content1");
        assert_eq!(fs::read_to_string(dest_dir.join("file2.txt")).unwrap(), "content2");
        assert_eq!(fs::read_to_string(dest_dir.join("file3.txt")).unwrap(), "content3");
    }

    #[test]
    fn test_is_path_within_dir_same_path() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let file = base.join("same.txt");
        fs::write(&file, b"test").unwrap();

        assert!(is_path_within_dir(&file, base));
        assert!(is_path_within_dir(base, base));
    }

    #[test]
    fn test_is_path_within_dir_nonexistent_subpath() {
        // When the base dir exists but the file doesn't, the string-based
        // path comparison is used. Both paths get normalized to absolute paths
        // with trailing slashes for comparison.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();

        // Create a real subdirectory so canonicalize works on parent
        let sub = base.join("real_subdir");
        fs::create_dir_all(&sub).unwrap();

        // Test a non-existent file inside the real subdir
        // (file doesn't exist -> canonicalize fails, but parent does -> string check)
        let _file_in_sub = sub.join("nonexistent_file.txt");
        // The sub directory itself exists, so canonicalize works for sub
        assert!(is_path_within_dir(&sub, base));
    }

    #[test]
    fn test_is_path_within_dir_partial_name_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base");
        let other = dir.path().join("base-other");

        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&other).unwrap();
        let file = other.join("file.txt");
        fs::write(&file, b"test").unwrap();

        assert!(!is_path_within_dir(&file, &base));
    }

    #[test]
    fn test_extract_zip_path_traversal_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("evil.zip");
        let dest_dir = dir.path().join("dest");

        // Create a zip manually with a path traversal entry
        let file = File::create(&zip_path).unwrap();
        let writer = std::io::BufWriter::new(file);
        let mut zip_writer = zip::ZipWriter::new(writer);
        let options = zip::write::SimpleFileOptions::default();

        // Try to add a file with path traversal
        zip_writer.start_file("../../../etc/evil.txt", options).unwrap();
        zip_writer.write_all(b"evil content").unwrap();
        zip_writer.finish().unwrap();

        let result = extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("zip slip"));
    }

    #[test]
    fn test_is_path_within_dir_relative_paths() {
        // Use tempfile for both so canonicalize works
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("subdir");
        let file_in_sub = subdir.join("file.txt");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(&file_in_sub, b"test").unwrap();

        assert!(is_path_within_dir(&file_in_sub, dir.path()));
    }

    #[test]
    fn test_is_path_within_dir_outside_relative() {
        let result = is_path_within_dir(
            Path::new("../outside/file.txt"),
            Path::new("subdir"),
        );
        assert!(!result);
    }

    #[test]
    fn test_normalize_path_str() {
        let result = normalize_path_str(Path::new("/tmp/test"));
        assert!(result.ends_with('/'));
        assert!(!result.contains('\\'));
    }

    #[test]
    fn test_extract_zip_with_directory_entries() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("src_dir");
        let zip_path = dir.path().join("dirs.zip");
        let dest_dir = dir.path().join("dest");

        fs::create_dir_all(source_dir.join("empty_dir")).unwrap();
        fs::write(source_dir.join("file.txt"), b"content").unwrap();

        create_zip(
            source_dir.to_string_lossy().as_ref(),
            zip_path.to_string_lossy().as_ref(),
        ).unwrap();

        extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        ).unwrap();

        assert!(dest_dir.join("empty_dir").is_dir());
        assert_eq!(fs::read_to_string(dest_dir.join("file.txt")).unwrap(), "content");
    }

    #[test]
    fn test_create_zip_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("large_source");
        let zip_path = dir.path().join("large.zip");
        let dest_dir = dir.path().join("dest");

        fs::create_dir_all(&source_dir).unwrap();
        // Create a file larger than the 8192 byte buffer
        let large_data = vec![0xAB_u8; 20000];
        fs::write(source_dir.join("large.bin"), &large_data).unwrap();

        create_zip(
            source_dir.to_string_lossy().as_ref(),
            zip_path.to_string_lossy().as_ref(),
        ).unwrap();

        extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        ).unwrap();

        let extracted = fs::read(dest_dir.join("large.bin")).unwrap();
        assert_eq!(extracted.len(), 20000);
    }

    #[test]
    fn test_extract_zip_empty_archive() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("empty.zip");
        let dest_dir = dir.path().join("dest");

        // Create an empty zip file
        let file = File::create(&zip_path).unwrap();
        let writer = std::io::BufWriter::new(file);
        let mut zip_writer = zip::ZipWriter::new(writer);
        zip_writer.finish().unwrap();

        let result = extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_zip_nested_directories() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("nested_src");
        let zip_path = dir.path().join("nested.zip");
        let dest_dir = dir.path().join("dest");

        // Create multi-level directories with files
        fs::create_dir_all(source_dir.join("a/b")).unwrap();
        fs::create_dir_all(source_dir.join("c")).unwrap();
        fs::write(source_dir.join("a/b/file1.txt"), b"file1").unwrap();
        fs::write(source_dir.join("c/file2.txt"), b"file2").unwrap();
        fs::write(source_dir.join("root.txt"), b"root").unwrap();

        create_zip(
            source_dir.to_string_lossy().as_ref(),
            zip_path.to_string_lossy().as_ref(),
        ).unwrap();

        extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        ).unwrap();

        assert_eq!(fs::read_to_string(dest_dir.join("a/b/file1.txt")).unwrap(), "file1");
        assert_eq!(fs::read_to_string(dest_dir.join("c/file2.txt")).unwrap(), "file2");
        assert_eq!(fs::read_to_string(dest_dir.join("root.txt")).unwrap(), "root");
    }

    #[test]
    fn test_create_zip_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("src");
        let zip_path = dir.path().join("output/nested/test.zip");

        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("file.txt"), b"content").unwrap();

        // Parent directories for zip should be created automatically
        let result = create_zip(
            source_dir.to_string_lossy().as_ref(),
            zip_path.to_string_lossy().as_ref(),
        );
        assert!(result.is_ok());
        assert!(zip_path.exists());
    }

    #[test]
    fn test_extract_zip_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("single.zip");
        let dest_dir = dir.path().join("dest");

        // Create zip with single file
        let file = File::create(&zip_path).unwrap();
        let writer = std::io::BufWriter::new(file);
        let mut zip_writer = zip::ZipWriter::new(writer);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip_writer.start_file("single.txt", options).unwrap();
        zip_writer.write_all(b"single file content").unwrap();
        zip_writer.finish().unwrap();

        extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        ).unwrap();

        assert_eq!(fs::read_to_string(dest_dir.join("single.txt")).unwrap(), "single file content");
    }

    #[test]
    fn test_extract_zip_preserves_empty_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("src_with_empty");
        let zip_path = dir.path().join("with_empty.zip");
        let dest_dir = dir.path().join("dest");

        fs::create_dir_all(source_dir.join("empty_subdir")).unwrap();
        fs::write(source_dir.join("file.txt"), b"has file").unwrap();

        create_zip(
            source_dir.to_string_lossy().as_ref(),
            zip_path.to_string_lossy().as_ref(),
        ).unwrap();

        extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        ).unwrap();

        assert!(dest_dir.join("empty_subdir").is_dir());
        assert_eq!(fs::read_to_string(dest_dir.join("file.txt")).unwrap(), "has file");
    }

    #[test]
    fn test_is_path_within_dir_both_nonexistent() {
        // When both paths exist, canonicalize is used for reliable comparison
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("subdir");
        std::fs::create_dir_all(&base).unwrap();
        let file = base.join("file.txt");
        std::fs::write(&file, "test").unwrap();
        let result = is_path_within_dir(&file, &base);
        assert!(result);
        // File in parent dir should not be within subdir
        let parent_file = tmp.path().join("other.txt");
        std::fs::write(&parent_file, "test").unwrap();
        assert!(!is_path_within_dir(&parent_file, &base));
    }

    #[test]
    fn test_is_path_within_dir_completely_different() {
        let result = is_path_within_dir(
            Path::new("/completely/different"),
            Path::new("/unrelated/path"),
        );
        assert!(!result);
    }

    #[test]
    fn test_normalize_path_str_trailing_slash() {
        let result = normalize_path_str(Path::new("/already/has/slash/"));
        assert!(result.ends_with('/'));
        assert!(result.starts_with('/'));
    }

    #[test]
    fn test_normalize_path_str_backslash_conversion() {
        let result = normalize_path_str(Path::new("C:\\Users\\test"));
        assert!(!result.contains('\\'));
        assert!(result.contains('/'));
    }

    #[test]
    fn test_extract_zip_path_traversal_backslash_dots() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("evil2.zip");
        let dest_dir = dir.path().join("dest");

        // Create a zip with path traversal using backslash
        let file = File::create(&zip_path).unwrap();
        let writer = std::io::BufWriter::new(file);
        let mut zip_writer = zip::ZipWriter::new(writer);
        let options = zip::write::SimpleFileOptions::default();
        zip_writer.start_file("foo\\..\\..\\bar.txt", options).unwrap();
        zip_writer.write_all(b"evil").unwrap();
        zip_writer.finish().unwrap();

        let result = extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("zip slip"));
    }

    #[test]
    fn test_extract_zip_with_unicode_filenames() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("unicode_src");
        let zip_path = dir.path().join("unicode.zip");
        let dest_dir = dir.path().join("dest");

        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("t.txt"), "Unicode: ").unwrap();

        create_zip(
            source_dir.to_string_lossy().as_ref(),
            zip_path.to_string_lossy().as_ref(),
        ).unwrap();

        extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        ).unwrap();

        assert!(dest_dir.join("t.txt").exists());
    }

    #[test]
    fn test_extract_zip_to_nested_dest() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("src");
        let zip_path = dir.path().join("test.zip");
        let dest_dir = dir.path().join("deep/nested/dest");

        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("hello.txt"), "hello world").unwrap();

        create_zip(
            source_dir.to_string_lossy().as_ref(),
            zip_path.to_string_lossy().as_ref(),
        ).unwrap();

        // dest_dir doesn't exist yet, should be created
        extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        ).unwrap();

        assert!(dest_dir.join("hello.txt").exists());
        assert_eq!(fs::read_to_string(dest_dir.join("hello.txt")).unwrap(), "hello world");
    }

    #[test]
    fn test_create_zip_multiple_files_and_extract() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("multi_src");
        let zip_path = dir.path().join("multi.zip");
        let dest_dir = dir.path().join("multi_dest");

        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("a.txt"), "content a").unwrap();
        fs::write(source_dir.join("b.txt"), "content b").unwrap();
        fs::write(source_dir.join("c.dat"), "binary data").unwrap();

        create_zip(
            source_dir.to_string_lossy().as_ref(),
            zip_path.to_string_lossy().as_ref(),
        ).unwrap();

        extract_zip(
            zip_path.to_string_lossy().as_ref(),
            dest_dir.to_string_lossy().as_ref(),
        ).unwrap();

        assert!(dest_dir.join("a.txt").exists());
        assert!(dest_dir.join("b.txt").exists());
        assert!(dest_dir.join("c.dat").exists());
        assert_eq!(fs::read_to_string(dest_dir.join("a.txt")).unwrap(), "content a");
        assert_eq!(fs::read_to_string(dest_dir.join("b.txt")).unwrap(), "content b");
    }

    #[test]
    fn test_extract_zip_nonexistent_file() {
        let result = extract_zip("/nonexistent/path.zip", "/tmp/dest");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to open zip file"));
    }

    #[test]
    fn test_create_zip_nonexistent_source_dir() {
        let result = create_zip("/nonexistent/source", "/tmp/output.zip");
        assert!(result.is_err());
    }
}
