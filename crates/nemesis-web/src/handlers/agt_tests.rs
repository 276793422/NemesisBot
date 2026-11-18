//! handlers/mod.rs AGT 覆盖率批次（2026-09-25）：workspace 文件三件套的
//! 确定性失败臂——read_workspace_file 缺文件（369）、write_workspace_file
//! 的 create_dir_all 失败（375：父路径段被同名文件占据）+ 原子写成功回读
//! （283 直线段）、resolve_path 绝对路径拒绝（279-281）。

use super::*;

#[test]
fn agt_read_workspace_file_missing_maps_to_read_error() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let err = read_workspace_file(&ws, "agt-nope.md").unwrap_err();
    assert!(err.contains("failed to read agt-nope.md"), "err: {err}");
}

#[test]
fn agt_write_workspace_file_dir_create_failure_maps_to_error() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    // `<ws>/sub` 是普通文件 → 写 `<ws>/sub/note.md` 时 create_dir_all(父)
    // 必炸（Win/Unix 同型：存在同名非目录）。
    let blocker = dir.path().join("sub");
    std::fs::write(&blocker, "not a dir").unwrap();
    let err = write_workspace_file(&ws, "sub/note.md", "x").unwrap_err();
    assert!(err.contains("failed to create dir"), "err: {err}");
}

#[test]
fn agt_write_workspace_file_atomic_roundtrip_and_absolute_reject() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();

    // 嵌套新目录：parent 建目录 + tmp+rename 原子落盘 + 回读一致。
    write_workspace_file(&ws, "agt-dir/note.md", "第一行\n第二行").unwrap();
    let back = read_workspace_file(&ws, "agt-dir/note.md").unwrap();
    assert_eq!(back, "第一行\n第二行");

    // resolve_path 防线：绝对路径（正斜杠头）显式拒绝。
    assert_eq!(
        resolve_path(&ws, "/abs.txt").unwrap_err(),
        "absolute paths not allowed"
    );
    // 盘符绝对路径是 Windows 形态——Linux 上 "C:/abs.txt" 是合法相对路径
    // （无盘符概念，Linux CI 实证假红），照 约定按平台门控。
    #[cfg(windows)]
    assert_eq!(
        resolve_path(&ws, "C:/abs.txt").unwrap_err(),
        "absolute paths not allowed"
    );
}

/// BUG-20261119-179：read_workspace_file 剥前导 UTF-8 BOM（记事本保存
/// 形态）——零宽 BOM 不进前端编辑器内容与回写；无 BOM 文件字节不变。
#[test]
fn agt_read_workspace_file_strips_leading_bom() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();

    // 带 BOM 文件：读回无 BOM。
    std::fs::write(dir.path().join("bom.md"), "\u{feff}# Title\nbody").unwrap();
    let back = read_workspace_file(&ws, "bom.md").unwrap();
    assert_eq!(back, "# Title\nbody", "leading BOM must be stripped");

    // 无 BOM 文件：字节原样（strip_prefix 不命中零拷贝路径）。
    std::fs::write(dir.path().join("plain.md"), "# Title\nbody").unwrap();
    let back = read_workspace_file(&ws, "plain.md").unwrap();
    assert_eq!(back, "# Title\nbody");
}
