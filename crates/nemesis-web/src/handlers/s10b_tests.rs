//! S10b (quality-hardening goal 冲刺, web 批次 2): handlers:: shared path/file
//! utility arms the gated `mod tests` skips — absolute-path rejection, the
//! canonicalize-fallback branch (non-existent base), `..` denial when the
//! target is missing, atomic-write fallback on a read-only destination, and
//! list_workspace_dir sorting/missing arms.

use super::*;

#[test]
fn resolve_path_rejects_absolute_forms() {
    let err = resolve_path("/ws", "/etc/passwd").unwrap_err();
    assert_eq!(err, "absolute paths not allowed");
    // 平台差异断言（2026-09-01 远端首跑暴露）：
    // - `C:/windows/abs.txt` 只在 Windows 是 is_absolute()；Linux 上是普通
    //   相对路径（文件名含 `C:` 而已），resolve_path 放行是正确行为。
    // - `\rooted` 在**两平台都 Err**：resolve_path 对 `\` 前缀有显式字符串
    //   检查（防 Windows 风格 rooted 路径，跨平台防御，handlers/mod.rs），
    //   与 Path::is_absolute 无关。
    if cfg!(windows) {
        assert!(resolve_path("/ws", "C:/windows/abs.txt").is_err());
    } else {
        assert!(resolve_path("/ws", "C:/windows/abs.txt").is_ok());
    }
    assert!(resolve_path("/ws", "\\rooted").is_err());
    assert!(resolve_path("/ws", "plain/relative.txt").is_ok());
}

#[test]
fn resolve_path_missing_base_skips_canonical_compare() {
    // Workspace does not exist on disk → canonicalize falls back to the raw
    // path; a clean relative stays allowed, `..` is denied by the string check.
    let base = std::env::temp_dir().join("s10b-no-such-ws-98127");
    let ws = base.to_string_lossy().to_string();
    assert!(!base.exists());

    let ok = resolve_path(&ws, "sub/file.txt").unwrap();
    assert!(ok.ends_with("sub\\file.txt") || ok.ends_with("sub/file.txt"));

    let err = resolve_path(&ws, "sub/../../escape.txt").unwrap_err();
    assert_eq!(err, "path traversal denied");
}

#[test]
fn write_workspace_file_falls_back_when_rename_fails_on_readonly_target() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    write_workspace_file(&ws, "ro.txt", "first").unwrap();

    let target = dir.path().join("ro.txt");
    let mut perms = std::fs::metadata(&target).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&target, perms).unwrap();

    let res = write_workspace_file(&ws, "ro.txt", "second");

    let mut perms = std::fs::metadata(&target).unwrap().permissions();
    perms.set_readonly(false);
    std::fs::set_permissions(&target, perms).unwrap();

    if cfg!(windows) {
        // tmp→target rename fails on the read-only destination; the fallback
        // direct write fails too → error surfaces.
        assert!(res.is_err());
        let content = std::fs::read_to_string(&target).unwrap();
        assert_eq!(content, "first", "failed write leaves original intact");
    } else {
        assert!(res.is_ok());
        assert_eq!(read_workspace_file(&ws, "ro.txt").unwrap(), "second");
    }
}

#[test]
fn list_workspace_dir_missing_empty_and_sorted_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();

    assert!(list_workspace_dir(&ws, "nope").unwrap().is_empty());

    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::write(dir.path().join("data/b.txt"), b"").unwrap();
    std::fs::write(dir.path().join("data/a.txt"), b"").unwrap();
    std::fs::create_dir_all(dir.path().join("data/zdir")).unwrap();
    assert_eq!(
        list_workspace_dir(&ws, "data").unwrap(),
        vec!["a.txt".to_string(), "b.txt".to_string(), "zdir".to_string()]
    );
}
