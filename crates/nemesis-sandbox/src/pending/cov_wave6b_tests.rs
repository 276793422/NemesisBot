//! pending.rs 覆盖率收尾（Wave6B）：`walk` 入口的上限早退臂（out 预填满
//! MAX_BOX_FILES → 首探即返回）与 `commit_file` 的 real_path 无父目录臂。
//!
//! 全部临时目录 + 不存在的路径，零系统副作用。

use super::*;
use std::path::Path;

/// out 预填满 → walk 入口上限检查立即返回（72-73 早退臂；5050 文件用例
/// 只能走到循环内的同款检查，入口臂需要预填）。
#[test]
fn walk_entry_cap_early_returns_on_prefilled_out() {
    let mut out: Vec<PendingFile> = (0..MAX_BOX_FILES)
        .map(|i| PendingFile {
            box_path: std::path::PathBuf::from(format!(r"Z:\box\f{i}")),
            real_path: std::path::PathBuf::from(format!(r"C:\real\f{i}")),
            size: 0,
        })
        .collect();

    // 目录不存在也无所谓：入口上限检查先于任何 IO。
    walk(
        Path::new(r"Z:\covw6b_missing_root"),
        Path::new(r"Z:\covw6b_missing_root"),
        Path::new(r"C:\Users\x"),
        &mut out,
    )
    .expect("入口上限早退必须是 Ok");

    assert_eq!(out.len(), MAX_BOX_FILES, "早退臂不得增删条目");
}

/// real_path 无父目录（盘根）→ 跳过 create_dir_all → copy 因源不存在而
/// 失败 → 带 "commit" context 传播（128 收口臂：if let Some(parent) 的
/// None 形态）。
#[test]
fn commit_file_without_parent_skips_mkdir_and_reports_copy_error() {
    let pending = PendingFile {
        box_path: std::path::PathBuf::from(r"Z:\covw6b_no_such_box_file.bin"),
        real_path: std::path::PathBuf::from(r"C:\"),
        size: 0,
    };
    let err = commit_file(&pending).expect_err("源不存在必须失败");
    let msg = format!("{err:#}");
    assert!(msg.contains("commit"), "必须带 commit context: {msg}");
}
