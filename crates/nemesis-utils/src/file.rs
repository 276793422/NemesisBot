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

    tmp_file.write_all(data).map_err(|e| format!("write temp: {}", e))?;
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
mod tests {
    use super::*;

    #[test]
    fn test_write_file_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt").to_string_lossy().to_string();
        write_file_atomic(&path, b"hello world", 0o644).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_ensure_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/c").to_string_lossy().to_string();
        ensure_dir(&path).unwrap();
        assert!(Path::new(&path).is_dir());
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_write_file_atomic_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("overwrite.txt").to_string_lossy().to_string();

        write_file_atomic(&path, b"first", 0o644).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");

        write_file_atomic(&path, b"second", 0o644).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
    }

    #[test]
    fn test_write_file_atomic_empty_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.txt").to_string_lossy().to_string();

        write_file_atomic(&path, b"", 0o644).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn test_write_file_atomic_large_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.bin").to_string_lossy().to_string();
        let large_data: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();

        write_file_atomic(&path, &large_data, 0o644).unwrap();
        let read_back = fs::read(&path).unwrap();
        assert_eq!(read_back, large_data);
    }

    #[test]
    fn test_write_file_atomic_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep/nested/dir/file.txt").to_string_lossy().to_string();

        write_file_atomic(&path, b"nested content", 0o644).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "nested content");
    }

    #[test]
    fn test_write_file_atomic_no_temp_file_left() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt").to_string_lossy().to_string();

        write_file_atomic(&path, b"content", 0o644).unwrap();

        // No .tmp files should remain
        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_name().to_string_lossy(), "file.txt");
    }

    #[test]
    fn test_read_file_string_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("read.txt").to_string_lossy().to_string();
        fs::write(&path, "readable content").unwrap();

        let content = read_file_string(&path).unwrap();
        assert_eq!(content, "readable content");
    }

    #[test]
    fn test_read_file_string_not_found() {
        let result = read_file_string("/nonexistent/file.txt");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("read:"));
    }

    #[test]
    fn test_ensure_dir_already_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();

        // Should succeed even if dir already exists
        ensure_dir(&path).unwrap();
        assert!(Path::new(&path).is_dir());
    }

    #[test]
    fn test_ensure_dir_nested() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("level1/level2/level3").to_string_lossy().to_string();

        ensure_dir(&path).unwrap();
        assert!(Path::new(&path).is_dir());
    }

    #[test]
    fn test_write_file_atomic_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("utf8.txt").to_string_lossy().to_string();
        let utf8_content = "Hello, World!";

        write_file_atomic(&path, utf8_content.as_bytes(), 0o644).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), utf8_content);
    }

    #[test]
    fn test_write_file_atomic_invalid_path() {
        // Writing to a path with a null byte should fail
        let result = write_file_atomic("/\0invalid/path.txt", b"data", 0o644);
        assert!(result.is_err());
    }

    #[test]
    fn test_write_file_atomic_root_file() {
        // Writing a file in current directory (no parent subpath needed)
        let dir = tempfile::tempdir().unwrap();
        // Use the tempdir itself as the parent; file directly inside
        let path = dir.path().join("root_file.txt").to_string_lossy().to_string();
        write_file_atomic(&path, b"root content", 0o644).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "root content");
    }

    #[test]
    fn test_write_file_atomic_binary_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("binary.dat").to_string_lossy().to_string();
        // All byte values
        let data: Vec<u8> = (0..=255).collect();
        write_file_atomic(&path, &data, 0o644).unwrap();
        let read_back = fs::read(&path).unwrap();
        assert_eq!(read_back.len(), 256);
        assert_eq!(read_back, data);
    }

    #[test]
    fn test_ensure_dir_invalid_path() {
        let result = ensure_dir("/\0bad/path");
        assert!(result.is_err());
    }

    #[test]
    fn test_read_file_string_binary_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("binary.dat").to_string_lossy().to_string();
        // Write invalid UTF-8
        fs::write(&path, &[0xFF, 0xFE, 0xFD]).unwrap();
        let result = read_file_string(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_write_file_atomic_concurrent() {
        // Verify that two sequential atomic writes to same file work correctly
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("concurrent.txt").to_string_lossy().to_string();

        let threads: Vec<_> = (0..5)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let data = format!("thread-{}", i);
                    write_file_atomic(&path, data.as_bytes(), 0o644)
                })
            })
            .collect();

        for t in threads {
            t.join().unwrap().unwrap();
        }

        // File should exist and contain one of the thread values
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("thread-"));
    }
}
