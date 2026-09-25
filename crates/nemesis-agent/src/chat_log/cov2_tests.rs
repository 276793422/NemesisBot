//! chat_log 覆盖率收尾批次（Wave6B）：读/截断/复制的 IO 错误分支 +
//! whole-log 滑窗 + meta/boundary sidecar 写失败 + 启动平化迁移各分支。
//!
//! 遵循本目录既有惯例：唯一会话键前缀 + 测试后清理（home 由 lib 测试二进制
//! 全局 OnceLock 固化，无法 per-test 重定向；唯一键保证不碰真实会话数据）。
//! Windows 句柄注入测试用 `#[cfg(windows)]` 单测门控。

use super::*;
use std::fs;
use std::path::Path;

fn cov_key(tag: &str) -> String {
    format!("test:covw6b:{tag}")
}

/// 以 share_mode(0)（拒绝一切共享）打开句柄——阻塞他人 File::open。
#[cfg(windows)]
fn lock_exclusive(path: &Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

/// 以 share_mode(1)（仅 FILE_SHARE_READ）打开句柄——阻塞他人写/删/改名。
#[cfg(windows)]
fn lock_shared_read_only(path: &Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .unwrap()
}

fn stored_user_msg(content: &str, ts: i64) -> crate::session::StoredMessage {
    crate::session::StoredMessage {
        role: "user".to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: ts.to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// read_chat_log
// ---------------------------------------------------------------------------

/// 打不开日志文件（共享冲突）→ 按缺失处理，返回空页（错误臂）。
#[cfg(windows)]
#[test]
fn read_chat_log_open_failure_returns_empty_page() {
    let key = cov_key("read_open_err");
    delete_chat_log(&key);
    append_chat_log(&key, "user", "hello");
    let path = log_path(&key);
    let _handle = lock_exclusive(&path);
    let (page, total, more, oldest) = read_chat_log(&key, 10, None);
    assert!(page.is_empty());
    assert_eq!(total, 0);
    assert!(!more);
    assert_eq!(oldest, 0);
}

/// whole-log 形态（limit ≥ 100_000）：解析行直接滑入窗口。
#[test]
fn read_chat_log_whole_log_window_eviction() {
    let key = cov_key("whole_log");
    let path = log_path(&key);
    let _ = fs::remove_file(&path);
    let mut body = String::new();
    for i in 0..100_002 {
        let row = serde_json::json!({"role":"user","content":format!("m{i}"),"timestamp":"t"});
        body.push_str(&row.to_string());
        body.push('\n');
    }
    fs::write(&path, body).unwrap();

    let (page, total, more, oldest) = read_chat_log(&key, 100_000, None);
    assert_eq!(total, 100_002);
    assert_eq!(page.len(), 100_000);
    assert!(more);
    assert_eq!(oldest, 2);
    // 窗口驱逐了最老两行：首页首行是 m2。
    assert_eq!(page[0]["content"].as_str(), Some("m2"));
    assert_eq!(page[99_999]["content"].as_str(), Some("m100001"));

    delete_chat_log(&key);
}

// ---------------------------------------------------------------------------
// truncate_chat_log_rows
// ---------------------------------------------------------------------------

/// tmp 文件被目录占位 → open 失败 → warn + 返回 0。
#[test]
fn truncate_returns_zero_when_tmp_unopenable() {
    let key = cov_key("trunc_tmp");
    delete_chat_log(&key);
    append_chat_log(&key, "user", "row");
    let tmp = log_path(&key).with_extension("jsonl.rewinding");
    fs::create_dir_all(&tmp).unwrap();

    let rows = vec![serde_json::json!({"role":"user","content":"keep"})];
    assert_eq!(truncate_chat_log_rows(&key, &rows), 0);

    fs::remove_dir(&tmp).unwrap();
    delete_chat_log(&key);
}

/// 目标日志是目录 → rename 失败 → warn + 清 tmp + 返回 0（原文件保留）。
/// 目录内塞守卫文件：Windows MoveFileEx(REPLACE_EXISTING) 偶发可替换
/// **空**目录——非空目录确定性拒绝（同 cov_tests 硬化）。
#[test]
fn truncate_returns_zero_when_rename_target_is_dir() {
    let key = cov_key("trunc_rename");
    let path = log_path(&key);
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("cov_guard"), b"x").unwrap();

    let rows = vec![serde_json::json!({"role":"user","content":"keep"})];
    assert_eq!(truncate_chat_log_rows(&key, &rows), 0);
    // tmp 已被清理。
    let tmp = path.with_extension("jsonl.rewinding");
    assert!(!tmp.exists());

    let _ = fs::remove_file(path.join("cov_guard"));
    fs::remove_dir(&path).unwrap();
}

// ---------------------------------------------------------------------------
// copy_chat_log_prefix / write_chat_log_from_store 的 open 失败臂
// ---------------------------------------------------------------------------

/// 目标路径被目录占用 → open 失败 → 复制 0 行（分支收口行）。
#[test]
fn copy_prefix_reports_lines_even_when_target_unopenable() {
    let src = cov_key("cpsrc");
    let dst = cov_key("cpdst");
    delete_chat_log(&src);
    delete_chat_log(&dst);
    append_chat_log(&src, "user", "q1");

    fs::create_dir_all(log_path(&dst)).unwrap();
    // lines 非空但落盘 open 失败：返回值仍是收集行数（宽容边界）。
    assert_eq!(copy_chat_log_prefix(&src, &dst, 1), 1);

    fs::remove_dir(log_path(&dst)).unwrap();
    delete_chat_log(&src);
}

#[test]
fn write_from_store_returns_zero_when_target_unopenable() {
    let dst = cov_key("fsdst");
    let _ = fs::remove_dir_all(log_path(&dst));
    fs::create_dir_all(log_path(&dst)).unwrap();

    let n = write_chat_log_from_store(&dst, &[stored_user_msg("q", 1)]);
    assert_eq!(n, 0);

    fs::remove_dir(log_path(&dst)).unwrap();
}

// ---------------------------------------------------------------------------
// clear_chat_log / boundary sidecar
// ---------------------------------------------------------------------------

/// boundary sidecar 写失败（共享冲突）→ warn 不 panic（主日志照常清空）。
#[cfg(windows)]
#[test]
fn clear_chat_log_warns_when_boundary_write_denied() {
    let key = cov_key("clear_bnd");
    delete_chat_log(&key);
    append_chat_log(&key, "user", "q");
    append_boundary_event(&key, "turn_start", "covw6");
    let bpath = boundary_path(&key);
    assert!(bpath.exists());

    let _handle = lock_shared_read_only(&bpath);
    clear_chat_log(&key); // 主日志写成功；bpath 写失败走 warn 臂。

    drop(_handle);
    let (page, _, _, _) = read_chat_log(&key, 10, None);
    assert!(page.is_empty());
    delete_chat_log(&key);
}

/// boundary 路径被目录占位 → open 失败 → warn + return。
#[test]
fn append_boundary_event_warns_when_path_is_dir() {
    let key = cov_key("bnd_dir");
    delete_chat_log(&key);
    let bp = boundary_path(&key);
    fs::create_dir_all(bp.parent().unwrap()).unwrap();
    fs::create_dir_all(&bp).unwrap();

    append_boundary_event(&key, "turn_start", "covw6"); // 只 warn 不 panic
    let events = read_boundary_events(&key);
    assert!(events.is_empty());

    fs::remove_dir(&bp).unwrap();
    delete_chat_log(&key);
}

// ---------------------------------------------------------------------------
// first_user_message 尾部 None
// ---------------------------------------------------------------------------

/// 只有 assistant 行 → 无非空 user 消息 → None。
#[test]
fn first_user_message_none_for_assistant_only_log() {
    let key = cov_key("assistant_only");
    delete_chat_log(&key);
    append_chat_log(&key, "assistant", "final answer");
    assert!(first_user_message(&key, 20).is_none());
    delete_chat_log(&key);
}

// ---------------------------------------------------------------------------
// meta sidecar 写失败臂
// ---------------------------------------------------------------------------

/// meta 写被拒 → clear_session_project 返回 false（warn 臂）。
#[cfg(windows)]
#[test]
fn clear_session_project_false_when_meta_write_denied() {
    let key = cov_key("proj_denied");
    delete_chat_log(&key);
    write_session_project(&key, "proj-1", "C:/covw6/proj");
    assert_eq!(read_session_meta(&key).as_deref(), None); // 无标题不回传

    let mpath = meta_path(&key);
    let _handle = lock_shared_read_only(&mpath);
    assert!(!clear_session_project(&key));

    drop(_handle);
    delete_chat_log(&key);
}

/// meta 写被拒 → clear_undelivered_replies 返回 false（warn 臂）。
#[cfg(windows)]
#[test]
fn clear_undelivered_false_when_meta_write_denied() {
    let key = cov_key("undel_denied");
    delete_chat_log(&key);
    mark_undelivered_reply(&key);
    mark_undelivered_reply(&key);

    let mpath = meta_path(&key);
    let _handle = lock_shared_read_only(&mpath);
    assert!(!clear_undelivered_replies(&key));

    drop(_handle);
    // 解锁后再清，验证计数确实非零过。
    assert!(clear_undelivered_replies(&key));
    delete_chat_log(&key);
}

// ---------------------------------------------------------------------------
// flatten_legacy_nested_dirs 各分支
// ---------------------------------------------------------------------------

#[test]
fn flatten_missing_root_is_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("no_such_root_covw6");
    super::flatten_legacy_nested_dirs(&missing); // 只能不 panic
}

#[test]
fn flatten_skips_subdirs_target_conflicts_and_rename_failures() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("logs");
    let child = root.join("nodeA");
    fs::create_dir_all(child.join("sub")).unwrap(); // 更深嵌套 → 跳过臂

    // ① 目标已存在 → 跳过臂。
    fs::write(child.join("dup.jsonl"), "x").unwrap();
    fs::write(root.join("nodeA_dup.jsonl"), "old").unwrap();

    // ② rename 失败（句柄锁）→ migration_log 臂。
    let locked = child.join("locked.jsonl");
    fs::write(&locked, "x").unwrap();

    // ③ 正常迁移（保证 moved>0 的收尾臂也走过）。
    fs::write(child.join("ok.jsonl"), "x").unwrap();

    #[cfg(windows)]
    let _handle = lock_shared_read_only(&locked);

    super::flatten_legacy_nested_dirs(&root);

    #[cfg(windows)]
    drop(_handle);

    assert!(root.join("nodeA_ok.jsonl").exists(), "正常文件应已平移");
    assert!(child.join("locked.jsonl").exists(), "锁定的文件留在原地");
    assert!(child.join("sub").is_dir(), "子目录不追");
    // 锁定文件还在 → 子目录非空 → 目录保留。
    assert!(child.is_dir());

    #[cfg(windows)]
    {
        drop(fs::remove_file(&locked));
    }
}

// ---------------------------------------------------------------------------
// migrate_nested_session_logs 公开入口（幂等零操作形态）
// ---------------------------------------------------------------------------

#[test]
fn migrate_entry_point_is_idempotent() {
    // 真实 home 的 sessions/boundary 目录已由启动平化过——二次调用零操作。
    migrate_nested_session_logs();
    migrate_nested_session_logs();
}
