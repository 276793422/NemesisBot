//! S9 覆盖率批次：spill.rs 剩余未覆盖行。
//! - 152-153/155/158：cleanup_expired 会话目录内过期文件 remove 失败
//!   （Windows readonly 文件 DeleteFile 拒绝；ReFS/Dev Drive 可能不执行
//!   → 探针先行，不执行则跳过断言）。
//! - 168-170：dir_empty 但 remove_dir 失败（空目录带 readonly 属性；
//!   同上探针门控）。
//! - 185-187/194：root 下散落文件的过期删除失败 warn。
//! - 90：spill_tool_result 的 `path.parent() None` 分支——join 出的路径
//!   恒有 parent，结构性不可达（见报告豁免组）。
//! - 102-103：write_all 中途失败 → 盘满/IO 错误级，机器依赖。

use super::*;
use crate::test_support::capture_logs;
use std::time::SystemTime;

fn temp_root(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "nemesis_spill_s9_{}_{}_{}",
        tag,
        std::process::id(),
        line!()
    ))
}

/// 把文件 mtime 拨到过去（std File::set_times，Rust 1.75+）。
fn age_file(path: &std::path::Path, old: SystemTime) {
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for set_times");
    f.set_times(
        std::fs::FileTimes::new()
            .set_accessed(old)
            .set_modified(old),
    )
    .expect("set_times");
}

fn set_readonly(path: &std::path::Path) -> bool {
    let meta = std::fs::metadata(path).expect("metadata");
    let mut perm = meta.permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(path, perm).is_ok()
}

/// 探针：本机文件系统是否执行 readonly-不可删语义（ReFS/Dev Drive 可能
/// 不执行——沿用 spill 既有测试的探针模式）。
fn readonly_delete_enforced(path: &std::path::Path) -> bool {
    let probe = path.with_extension("probe_s9");
    std::fs::write(&probe, "x").unwrap();
    set_readonly(&probe);
    let blocked = std::fs::remove_file(&probe).is_err();
    if !blocked {
        let _ = std::fs::remove_file(&probe);
    }
    blocked
}

const OLD: SystemTime = SystemTime::UNIX_EPOCH;

/// 会话目录内：过期文件 remove 失败 → warn + dir_empty=false（152-158）；
/// 对照同目录另一过期文件正常删除。
#[test]
fn cleanup_expired_readonly_file_survives_with_warn() {
    let _logs = capture_logs();
    let root = temp_root("rofile");
    let _ = std::fs::remove_dir_all(&root);
    let sess = root.join("s9sess");
    std::fs::create_dir_all(&sess).unwrap();
    let keep = sess.join("keep.txt");
    let gone = sess.join("gone.txt");
    std::fs::write(&keep, "expired but readonly").unwrap();
    std::fs::write(&gone, "expired and deletable").unwrap();
    age_file(&keep, OLD);
    age_file(&gone, OLD);
    set_readonly(&keep);

    if !readonly_delete_enforced(&keep) {
        eprintln!("[s9] fs does not enforce readonly-delete; skipping");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }

    let deleted = cleanup_expired(&root, 1);
    assert_eq!(deleted, 1, "only the writable file deleted");
    assert!(keep.exists(), "readonly file survived");
    assert!(!gone.exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// 空会话目录带 readonly 属性 → remove_dir 失败 → warn（166-172）。
/// 注意 readonly 属性要先在目录有文件时设（Windows 目录 readonly 语义）。
#[test]
fn cleanup_expired_readonly_dir_warns_on_remove_dir() {
    let _logs = capture_logs();
    let root = temp_root("rodir");
    let _ = std::fs::remove_dir_all(&root);
    let sess = root.join("s9sess_ro");
    std::fs::create_dir_all(&sess).unwrap();
    // 目录里先放一个文件，把目录设为 readonly，再删除文件（绕开
    // Windows 上对空目录设属性的一些怪癖），使 read_dir 可列、remove_dir
    // 被拒。
    let filler = sess.join("filler.txt");
    std::fs::write(&filler, "x").unwrap();
    set_readonly(&sess);
    // Windows：目录 readonly 属性不拦内部文件删除；Linux（chmod 555）拦。
    // → 删除被拒就先恢复可写删掉再设回，两平台最终态一致：空 + 只读目录。
    if std::fs::remove_file(&filler).is_err() {
        let mut perm = std::fs::metadata(&sess).unwrap().permissions();
        perm.set_readonly(false);
        std::fs::set_permissions(&sess, perm).unwrap();
        std::fs::remove_file(&filler).unwrap();
        set_readonly(&sess);
    }

    // 探针：readonly 目录的 remove_dir 是否被拒
    let blocked = std::fs::remove_dir(&sess).is_err();
    if !blocked {
        let _ = std::fs::remove_dir(&sess);
        eprintln!("[s9] fs does not enforce readonly-rmdir; skipping");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }

    let deleted = cleanup_expired(&root, 1);
    assert_eq!(deleted, 0);
    assert!(sess.exists(), "readonly dir survived remove_dir");
    let _ = std::fs::remove_dir_all(&root);
}

/// root 下散落文件：过期 + readonly → 删失败 warn（183-189）；对照过期可
/// 写文件正常删（190-192）；新鲜文件保留（161-162 同构路径）。
#[test]
fn cleanup_expired_stray_files_at_root() {
    let _logs = capture_logs();
    let root = temp_root("stray");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let stray_keep = root.join("stray_keep.txt");
    let stray_gone = root.join("stray_gone.txt");
    let stray_fresh = root.join("stray_fresh.txt");
    std::fs::write(&stray_keep, "ro").unwrap();
    std::fs::write(&stray_gone, "ok").unwrap();
    std::fs::write(&stray_fresh, "fresh").unwrap();
    age_file(&stray_keep, OLD);
    age_file(&stray_gone, OLD);
    // stray_fresh 保持 now mtime
    set_readonly(&stray_keep);

    if !readonly_delete_enforced(&stray_keep) {
        eprintln!("[s9] fs does not enforce readonly-delete; skipping");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }

    let deleted = cleanup_expired(&root, 1);
    assert_eq!(deleted, 1, "writable expired stray deleted");
    assert!(stray_keep.exists(), "readonly stray survived");
    assert!(!stray_gone.exists());
    assert!(stray_fresh.exists(), "fresh stray kept");
    let _ = std::fs::remove_dir_all(&root);
}

/// 会话目录内嵌套目录 → dir_empty=false 且不动它（140-142 对照）；
/// 全部过期可删 → 目录一并删除（166-167 成功路径）。
#[test]
fn cleanup_expired_nested_dir_blocks_and_clean_sweep_removes_dir() {
    let root = temp_root("nested");
    let _ = std::fs::remove_dir_all(&root);
    let sess = root.join("s9nest");
    std::fs::create_dir_all(sess.join("inner")).unwrap();
    let f = sess.join("old.txt");
    std::fs::write(&f, "x").unwrap();
    age_file(&f, OLD);

    let deleted = cleanup_expired(&root, 1);
    assert_eq!(deleted, 1);
    assert!(sess.exists(), "nested dir blocks dir removal");
    assert!(sess.join("inner").exists());

    // 清掉嵌套目录后再跑 → 空目录被删（166-167 remove_dir 成功路径）
    std::fs::remove_dir_all(sess.join("inner")).unwrap();
    let deleted2 = cleanup_expired(&root, 1);
    assert_eq!(deleted2, 0, "nothing left to delete");
    assert!(!sess.exists(), "empty session dir removed on sweep");
    let _ = std::fs::remove_dir_all(&root);
}

/// 对照：retention_days=0 → 直接返回 0；root 不存在 → 0。
#[test]
fn cleanup_expired_disabled_and_missing_root() {
    let root = temp_root("disabled");
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(cleanup_expired(&root, 0), 0);
    assert_eq!(cleanup_expired(&root, 7), 0, "missing root → 0");
    let _ = std::fs::remove_dir_all(&root);
}
