//! `collect_relative_files` 单测：链接项跳过语义（BUG-20261029-138）。
//!
//! 修复前形态：`p.is_dir()` 是 `fs::metadata` 跟随链接版——工作区内
//! junction/symlink 被当目录递归，目标树文件以 `link/...` 前缀进 files
//! 清单（`files_exist` 断言面污染）；自指链接则无限递归挂死 runner
//! （进程内装配 = 挂死 gateway）。红证用指向树外真实目录的链接钉跟随
//! 语义（自指挂死不可断言，同一跟随语义的推论）；与 R204
//! （BUG-20261029-135）`label_tree` 修复同款判定（symlink_metadata）。

use super::*;

fn plant_fixture(base: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let ws = base.join("ws");
    let sub = ws.join("sub");
    std::fs::create_dir_all(&sub).expect("mkdir sub");
    std::fs::write(sub.join("real.txt"), b"x").expect("write real");
    let outside = base.join("outside");
    std::fs::create_dir_all(outside.join("inner")).expect("mkdir outside");
    std::fs::write(outside.join("inner").join("leak.txt"), b"y").expect("write leak");
    (ws, outside)
}

fn assert_link_skipped(files: &[String]) {
    assert!(
        !files.iter().any(|f| f.starts_with("link/")),
        "链接目标树文件不得进清单: {files:?}"
    );
    assert!(
        files.iter().any(|f| f == "sub/real.txt"),
        "真实文件应收集: {files:?}"
    );
}

/// Windows junction（mklink /J，无需特权）：不跟随不收集。
#[cfg(windows)]
#[test]
fn collect_relative_files_skips_junction_entries() {
    let base = tempfile::tempdir().expect("tempdir");
    let (ws, outside) = plant_fixture(base.path());
    let junc = ws.join("link");
    let out = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junc)
        .arg(&outside)
        .output()
        .expect("run mklink");
    assert!(
        out.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let mut files = Vec::new();
    collect_relative_files(&ws, &ws, &mut files);
    assert_link_skipped(&files);
}

/// Unix 目录 symlink：不跟随不收集（CI Linux nightly 实跑兜底）。
#[cfg(unix)]
#[test]
fn collect_relative_files_skips_symlink_entries() {
    let base = tempfile::tempdir().expect("tempdir");
    let (ws, outside) = plant_fixture(base.path());
    std::os::unix::fs::symlink(&outside, ws.join("link")).expect("symlink");

    let mut files = Vec::new();
    collect_relative_files(&ws, &ws, &mut files);
    assert_link_skipped(&files);
}
