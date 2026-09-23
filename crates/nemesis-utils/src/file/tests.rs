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
    let path = dir
        .path()
        .join("overwrite.txt")
        .to_string_lossy()
        .to_string();

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
    let path = dir
        .path()
        .join("deep/nested/dir/file.txt")
        .to_string_lossy()
        .to_string();

    write_file_atomic(&path, b"nested content", 0o644).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "nested content");
}

#[test]
fn test_write_file_atomic_no_temp_file_left() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt").to_string_lossy().to_string();

    write_file_atomic(&path, b"content", 0o644).unwrap();

    // No .tmp files should remain
    let entries: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
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
    let path = dir
        .path()
        .join("level1/level2/level3")
        .to_string_lossy()
        .to_string();

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
    let path = dir
        .path()
        .join("root_file.txt")
        .to_string_lossy()
        .to_string();
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
    fs::write(&path, [0xFF, 0xFE, 0xFD]).unwrap();
    let result = read_file_string(&path);
    assert!(result.is_err());
}

#[test]
fn test_write_file_atomic_concurrent() {
    // Verify that two sequential atomic writes to same file work correctly
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("concurrent.txt")
        .to_string_lossy()
        .to_string();

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

/// 空路径的 `Path::parent()` 返回 None → 落到 "." 分支。tmp 文件落在当前
/// 目录（"."），最终 rename 到 "" 确定性失败；helper 自身清理临时文件
/// （REL-002 失败清理保证），测试仍兜底清扫 CWD 残留。
#[test]
fn test_write_file_atomic_empty_path_skips_parent_creation() {
    let result = write_file_atomic("", b"data", 0o644);
    let err = result.unwrap_err();
    assert!(err.starts_with("atomic write"), "unexpected error: {err}");
    assert!(err.contains("rename"), "error should carry the step: {err}");

    // 清理本用例落在当前目录的临时文件（正常由 helper 清理，兜底防御）
    for entry in fs::read_dir(".").unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(".tmp-") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

// ============================================================
// REL-002（2026-09-23）：失败注入 / 并发 / 权限验收
// ============================================================

/// 替换失败臂：目标是已存在的目录 → rename 失败；原目录原样、无 tmp 残留。
#[test]
fn test_write_file_atomic_target_is_directory_fails_clean() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();

    let result = write_file_atomic(target.to_str().unwrap(), b"data", 0o644);
    assert!(result.is_err());

    // 原目标（目录）未被动过
    assert!(target.is_dir(), "original directory must be untouched");
    // 无临时文件残留
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files must be cleaned up: {:?}",
        leftovers.iter().map(|e| e.file_name()).collect::<Vec<_>>()
    );
}

/// 创建失败臂（unix）：父目录只读 → 临时文件建不出来 → 原文件完好。
#[cfg(unix)]
#[test]
fn test_write_file_atomic_readonly_dir_original_intact() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keep.txt");
    fs::write(&path, b"original").unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();

    let result = write_file_atomic(path.to_str().unwrap(), b"new", 0o644);
    // 恢复权限让 tempdir 能自清理
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();

    assert!(result.is_err(), "write into read-only dir must fail");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "original",
        "old config must survive a failed write"
    );
}

/// unix 权限在临时文件创建时即挂：0600 生效（验收「权限保持预期值」）。
#[cfg(unix)]
#[test]
fn test_write_file_atomic_unix_perm_applied() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret.cfg").to_string_lossy().to_string();
    write_file_atomic(&path, b"k=v", 0o600).unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "perm must be applied at tmp creation");
}

/// 并发写同一目标：终态必为某一次写入的完整内容（无交错半截），无 tmp 残留。
#[test]
fn test_write_file_atomic_concurrent_no_interleave() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race.cfg").to_string_lossy().to_string();

    let threads: Vec<_> = (0..8)
        .map(|i| {
            let path = path.clone();
            std::thread::spawn(move || {
                let payload = format!("payload-{i}-{}", "x".repeat(2048));
                write_file_atomic(&path, payload.as_bytes(), 0o644)
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap().unwrap();
    }

    let content = fs::read(&path).unwrap();
    let text = String::from_utf8(content).unwrap();
    assert!(
        text.starts_with("payload-") && text.ends_with(&"x".repeat(2048)),
        "final content must be ONE complete write, not interleaved"
    );

    let leftovers = fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
        .count();
    assert_eq!(leftovers, 0, "no temp files may survive concurrent writes");
}
