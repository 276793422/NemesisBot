//! chat_log 覆盖率收尾批次（Wave6B）：读/截断/复制的 IO 错误分支 +
//! whole-log 滑窗 + meta/boundary sidecar 写失败 + 启动平化迁移各分支。
//!
//! 遵循本目录既有惯例：唯一会话键前缀 + 测试后清理（home 由 lib 测试二进制
//! 全局 OnceLock 固化，无法 per-test 重定向；唯一键保证不碰真实会话数据）。
//! Windows 句柄注入测试用 `#[cfg(windows)]` 单测门控。

use super::*;
use std::fs;
// Path 类型只被下方 Windows 句柄锁 helper 引用——Linux 下门控防未用导入。
#[cfg(windows)]
use std::path::Path;

/// 进程级唯一键前缀（脆性修复，机制见 cov_tests.rs 同名 helper
/// 注释）：静态前缀跨运行复用同一路径，Windows 删除挂起残留会让下轮夹具
/// create_dir_all 撞 os error 183 自我延续红。pid + 启动纳秒后缀根治。
fn cov_key(tag: &str) -> String {
    static RUN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let run = RUN.get_or_init(|| {
        format!(
            "{:x}{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    });
    format!("test:covw6b:{run}:{tag}")
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
/// BUG-20261106-002 差分同场：全量读者契约 = `usize::MAX` 无驱逐
/// （>10 万行整卷返回，最旧行在首）；`limit=100_000` 则照旧从头驱逐。
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

    // BUG-20261106-002 差分：同一 >10 万行文件，usize::MAX = 全量读者
    // 契约——零驱逐、全部 100_002 行、最旧行在首、无后续页。
    let (all, total_all, more_all, oldest_all) = read_chat_log(&key, usize::MAX, None);
    assert_eq!(total_all, 100_002);
    assert_eq!(all.len(), 100_002);
    assert!(!more_all);
    assert_eq!(oldest_all, 0);
    assert_eq!(all[0]["content"].as_str(), Some("m0"));
    assert_eq!(all[100_001]["content"].as_str(), Some("m100001"));

    delete_chat_log(&key);
}

// ---------------------------------------------------------------------------
// truncate_chat_log_rows
// ---------------------------------------------------------------------------

/// 遗留崩溃残留（旧实现的 `.jsonl.rewinding` 中断目录、任何孤儿文件）
/// 不得阻塞截断——BUG-20261106-001 后 truncate 委托
/// `nemesis_utils::write_file_atomic`（自造唯一临时名），旧实现的中断
/// 残留退化为无害垃圾。旧测试在 `.rewinding` 路径建目录阻断 tmp open，
/// helper 下该路径不再是截断通道，测试改语义：残留在场时截断照常成功。
#[test]
fn truncate_succeeds_with_legacy_residue() {
    let key = cov_key("trunc_tmp");
    delete_chat_log(&key);
    append_chat_log(&key, "user", "row");
    let tmp = log_path(&key).with_extension("jsonl.rewinding");
    // 中断遗留的同路径文件会让 create_dir_all 死循环，先消干净。
    let _ = fs::remove_file(&tmp);
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    let rows = vec![serde_json::json!({"role":"user","content":"keep"})];
    assert_eq!(truncate_chat_log_rows(&key, &rows), 1);

    // 截断生效：重写后的文件只含 kept 行。
    let (page, _, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(page.len(), 1);
    assert_eq!(page[0]["content"].as_str(), Some("keep"));

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
    // 预清理对文件/目录都生效：上次运行中断可能在同路径留下**文件**
    // （remove_dir_all 对文件静默失败 → create_dir_all AlreadyExists 死循环）。
    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("cov_guard"), b"x").unwrap();

    let rows = vec![serde_json::json!({"role":"user","content":"keep"})];
    assert_eq!(truncate_chat_log_rows(&key, &rows), 0);
    // 失败后原目标（目录 + 守卫文件）原封不动——helper 原子性契约的
    // 失败臂：要么原样、要么截断后完好，绝不半截。
    assert!(path.join("cov_guard").exists(), "失败后原目标不得被动过");

    let _ = fs::remove_file(path.join("cov_guard"));
    fs::remove_dir(&path).unwrap();
}

// ---------------------------------------------------------------------------
// create_chat_log_exclusive / write_chat_log_rows（排他创建语义）
// ---------------------------------------------------------------------------

/// BUG-20261106-003：批次创建从 append 改排他（create_new）——已存在键
/// 必须诚实败北（Result 版回 AlreadyExists / usize 包装版归一 0），
/// 既有文件内容原封不动（此前 append 语义会把导入行追加进既有会话，
/// check-then-create 竞态窗口同理）。新键照常成功。
#[test]
fn exclusive_create_refuses_existing_target() {
    let key = cov_key("excl");
    delete_chat_log(&key);
    append_chat_log(&key, "user", "original");
    let rows = vec![serde_json::json!({"role":"user","content":"invader"})];

    match create_chat_log_exclusive(&key, &rows) {
        Err(ChatLogCreateError::AlreadyExists) => {}
        other => panic!("已存在键必须回 AlreadyExists，实际 {other:?}"),
    }
    let (page, total, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(total, 1, "既有文件不得被触碰");
    assert_eq!(page[0]["content"].as_str(), Some("original"));

    // usize 包装版（fork 消费）：同键归一 0 = fork 回退臂语义。
    assert_eq!(write_chat_log_rows(&key, &rows), 0);
    let (page, total, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(total, 1);
    assert_eq!(page[0]["content"].as_str(), Some("original"));

    // 新键：排他创建成功，行数如实。
    let fresh = cov_key("excl_fresh");
    delete_chat_log(&fresh);
    assert!(matches!(create_chat_log_exclusive(&fresh, &rows), Ok(1)));
    let (page, total, _, _) = read_chat_log(&fresh, 10, None);
    assert_eq!(total, 1);
    assert_eq!(page[0]["content"].as_str(), Some("invader"));

    delete_chat_log(&key);
    delete_chat_log(&fresh);
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
    let _ = clear_chat_log(&key); // 主日志写成功（bpath 写失败走 warn 臂，不影响 Ok）。

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

    // ② rename 失败（句柄锁）→ migration_log 臂。锁只在 Windows 成立
    // （POSIX rename 不受共享读锁阻挡 → Linux 下该文件照常平移）。
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
    // 锁定文件留原地仅 Windows 成立（Linux 无共享锁语义，locked 已平移）。
    #[cfg(windows)]
    assert!(child.join("locked.jsonl").exists(), "锁定的文件留在原地");
    assert!(child.join("sub").is_dir(), "子目录不追");
    // 两平台下 sub 都在 → 子目录非空 → 目录保留。
    assert!(child.is_dir());

    #[cfg(windows)]
    {
        drop(fs::remove_file(&locked));
    }
}

// ---------------------------------------------------------------------------
// migrate_nested_session_logs 公开入口
// ---------------------------------------------------------------------------
// 【不测公开入口】`migrate_nested_session_logs` 硬绑真实 home 的共享目录，
// 且平化会删除空目录——与并行测试在同目录创建的占位目录（本文件多处
// dir-squatter 形态）结构性互斥（实测：并行下吃掉别的测试的目录 → 4 个
// 测试 ~50% 假红）。平化行为本身已由上面两个 tempdir 根的 flatten 测试
// 覆盖；入口装配（gateway/acp/run/mcp_serve 调用）归各入口的集成测试。
