//! chat_log 覆盖率补充测试（全字段 meta 落盘回读 / 截断与行写原语 /
//! sidecar meta 各分支）。

use super::*;
use crate::r#loop::FileChangeKind;

/// 本文件的会话键前缀（唯一 + 测试后清理，遵循既有 tests.rs 惯例）。
fn cov_key(tag: &str) -> String {
    format!("test:cov_chatlog:{tag}")
}

/// 全字段 ChatLogMeta：cron 标记 / images / file_changes / checkpoint_turn
/// 全部落盘且回读可见（写侧字段完整性锚）。
#[test]
fn append_meta_writes_all_optional_fields() {
    let key = cov_key("meta_full");
    delete_chat_log(&key);

    let changes = vec![
        FileChange {
            path: "src/a.rs".to_string(),
            kind: FileChangeKind::Create,
        },
        FileChange {
            path: "src/a.rs".to_string(),
            kind: FileChangeKind::Modify,
        },
    ];
    let meta = ChatLogMeta {
        model: Some("prov/model-x"),
        cron_job_id: Some("job-42"),
        cron_job_name: Some("nightly"),
        images: &["/tmp/pic.png".to_string()],
        file_changes: &changes,
        checkpoint_turn: Some(7),
    };
    append_chat_log_meta(&key, "assistant", "done", &meta);

    let (msgs, total, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(total, 1);
    let row = &msgs[0];
    assert_eq!(row["cron_job_id"].as_str(), Some("job-42"));
    assert_eq!(row["cron_job_name"].as_str(), Some("nightly"));
    assert_eq!(
        row["images"][0].as_str(),
        Some("/tmp/pic.png"),
        "images: {:?}",
        row["images"]
    );
    assert_eq!(row["file_changes"][0]["path"].as_str(), Some("src/a.rs"));
    assert_eq!(row["file_changes"][1]["kind"].as_str(), Some("Modify"));
    assert_eq!(row["checkpoint_turn"].as_u64(), Some(7));

    // dedup：同 path 保留首次出现序、kind 取最后一次声明。
    let deduped = dedup_file_changes(changes);
    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].kind, FileChangeKind::Modify);

    delete_chat_log(&key);
}

/// append_chat_log_with_model_and_node：source_node 字段落盘。
#[test]
fn append_with_node_writes_source_node() {
    let key = cov_key("node");
    delete_chat_log(&key);
    append_chat_log_with_model_and_node(&key, "assistant", "from peer", None, Some("node-A"));
    let (msgs, _, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(msgs[0]["source_node"].as_str(), Some("node-A"));
    delete_chat_log(&key);
}

/// 空文件（存在但 0 行）→ 空读 + total=0。
#[test]
fn read_empty_log_file_returns_empty() {
    let key = cov_key("empty_file");
    delete_chat_log(&key);
    // 先写一行再清空，确保文件存在且为空。
    append_chat_log(&key, "user", "x");
    clear_chat_log(&key);
    let (msgs, total, has_more, oldest) = read_chat_log(&key, 10, None);
    assert!(msgs.is_empty());
    assert_eq!(total, 0);
    assert!(!has_more);
    assert_eq!(oldest, 0);
    delete_chat_log(&key);
}

/// 非法行（非 UTF-8）→ 计数含该行并在该处截断（旧 count 语义）。
#[test]
fn read_log_with_invalid_utf8_stops_at_bad_line() {
    let key = cov_key("bad_utf8");
    delete_chat_log(&key);
    append_chat_log(&key, "user", "good line");
    // 追加一段非法 UTF-8 字节。
    let path = log_path(&key);
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open");
    f.write_all(b"\xff\xfe\xfa\n").expect("write bad bytes");
    f.flush().ok();
    drop(f);

    let (msgs, total, _, _) = read_chat_log(&key, 50, None);
    assert_eq!(total, 2, "bad line counted");
    assert_eq!(msgs.len(), 1, "window stops at bad line");
    assert_eq!(msgs[0]["content"].as_str(), Some("good line"));
    delete_chat_log(&key);
}

/// write_chat_log_rows：写入行数返回 + 回读一致。
#[test]
fn write_chat_log_rows_roundtrip() {
    let key = cov_key("rows_write");
    delete_chat_log(&key);
    let rows = vec![
        serde_json::json!({"role": "user", "content": "r1"}),
        serde_json::json!({"role": "assistant", "content": "r2"}),
    ];
    let n = write_chat_log_rows(&key, &rows);
    assert_eq!(n, 2);
    let (msgs, total, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(total, 2);
    assert_eq!(msgs[1]["content"].as_str(), Some("r2"));
    delete_chat_log(&key);
}

/// write_chat_log_rows：目标路径是目录 → 打开失败 → 0（宽容边界）。
#[test]
fn write_chat_log_rows_unopenable_target_returns_zero() {
    let key = cov_key("rows_dir");
    delete_chat_log(&key);
    let target = log_path(&key);
    std::fs::create_dir_all(&target).expect("mkdir at log path");
    let rows = vec![serde_json::json!({"role": "user", "content": "x"})];
    assert_eq!(write_chat_log_rows(&key, &rows), 0);
    let _ = std::fs::remove_dir(&target);
}

/// truncate_chat_log_rows：原位截断为 kept 子集（VERBATIM）+ 空 kept 清空。
#[test]
fn truncate_chat_log_rows_keeps_subset_then_clears() {
    let key = cov_key("truncate");
    delete_chat_log(&key);
    append_chat_log(&key, "user", "t1");
    append_chat_log(&key, "assistant", "a1");
    append_chat_log(&key, "user", "t2");

    let (all, _, _, _) = read_chat_log(&key, 100, None);
    let n = truncate_chat_log_rows(&key, &all[..2]);
    assert_eq!(n, 2);
    let (msgs, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 2);
    assert_eq!(msgs[1]["content"].as_str(), Some("a1"));

    // 空 kept = 清空文件（文件保留）。
    let n2 = truncate_chat_log_rows(&key, &[]);
    assert_eq!(n2, 0);
    let (_, total2, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total2, 0);
    delete_chat_log(&key);
}

/// truncate_chat_log_rows：目标为目录 → rename 失败 → 0 且不留 tmp。
#[test]
fn truncate_rename_failure_returns_zero() {
    let key = cov_key("trunc_dir");
    delete_chat_log(&key);
    append_chat_log(&key, "user", "keep me");
    // 目标 log_path 是目录 → rename(tmp → path) 必败。
    let path = log_path(&key);
    // 先拿真实文件挪走，把同一路径换成目录。
    std::fs::remove_file(&path).expect("remove log");
    std::fs::create_dir_all(&path).expect("mkdir at log path");
    // 占位目录塞一个守卫文件：Windows MoveFileEx(REPLACE_EXISTING) 偶发
    // 能替换**空**目录（measure 高并载下实测复现一次）——非空目录确定
    // 性拒绝，钉死 rename 必败前提。
    std::fs::write(path.join("cov_guard"), b"x").expect("guard file");

    let kept = vec![serde_json::json!({"role": "user", "content": "x"})];
    assert_eq!(truncate_chat_log_rows(&key, &kept), 0);
    let _ = std::fs::remove_file(path.join("cov_guard"));
    let _ = std::fs::remove_dir(&path);
}

/// copy_chat_log_prefix：源不存在 / 无可复制行 → 0。
#[test]
fn copy_prefix_missing_source_returns_zero() {
    let src = cov_key("copy_missing_src");
    let dst = cov_key("copy_missing_dst");
    delete_chat_log(&src);
    delete_chat_log(&dst);
    assert_eq!(copy_chat_log_prefix(&src, &dst, 3), 0);
}

/// write_chat_log_from_store：tool 角色行不投影 → 全滤时返回 0。
#[test]
fn write_from_store_filters_non_projected_rows() {
    let key = cov_key("store_filter");
    delete_chat_log(&key);
    let tool_only = vec![stored_msg("tool", "tool output", 0)];
    assert_eq!(write_chat_log_from_store(&key, &tool_only), 0);

    let mixed = vec![
        stored_msg("user", "q", 1),
        // assistant 空内容 = 纯 tool_calls 中间行，不投影。
        stored_msg("assistant", "   ", 2),
        stored_msg("assistant", "final", 3),
    ];
    let n = write_chat_log_from_store(&key, &mixed);
    assert_eq!(n, 2);
    let (msgs, _, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1]["content"].as_str(), Some("final"));
    delete_chat_log(&key);
}

/// StoredMessage 全字段构造助手（无 Default derive）。
fn stored_msg(role: &str, content: &str, ts: i64) -> crate::session::StoredMessage {
    crate::session::StoredMessage {
        role: role.to_string(),
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

/// first_user_message：文件缺失 → None；命中时截到 max_chars（char 安全）。
#[test]
fn first_user_message_missing_and_truncation() {
    let key = cov_key("first_user");
    delete_chat_log(&key);
    assert!(first_user_message(&key, 10).is_none());

    append_chat_log(&key, "assistant", "早于 user 的行");
    append_chat_log(&key, "user", "  ");
    append_chat_log(&key, "user", "这是一个很长的首条用户消息用来测试截断边界");
    let got = first_user_message(&key, 5).expect("first user msg");
    assert_eq!(got.chars().count(), 5);
    delete_chat_log(&key);
}

/// fork 血缘 meta：upsert 保留 title；回读 full meta。
#[test]
fn session_parent_meta_roundtrip() {
    let key = cov_key("parent");
    delete_chat_log(&key);
    write_session_meta(&key, "标题A");
    write_session_parent(&key, "test:cov_chatlog:parent_src", 4);
    let meta = read_session_meta_full(&key).expect("meta present");
    assert_eq!(meta.title.as_deref(), Some("标题A"));
    assert_eq!(meta.parent.as_deref(), Some("test:cov_chatlog:parent_src"));
    assert_eq!(meta.forked_at_turn, Some(4));
    delete_chat_log(&key);
}

/// 项目归属：烧入 → 摘除 → 再摘除 no-op（false）；无 meta 摘除 no-op。
#[test]
fn session_project_burn_and_clear() {
    let key = cov_key("project");
    delete_chat_log(&key);
    // 无 meta 文件 → clear 是 no-op。
    assert!(!clear_session_project(&key));

    write_session_project(&key, "proj-1", "/ws/proj-1");
    let meta = read_session_meta_full(&key).expect("meta");
    assert_eq!(meta.project_id.as_deref(), Some("proj-1"));
    assert_eq!(meta.project_path.as_deref(), Some("/ws/proj-1"));

    assert!(clear_session_project(&key), "first clear removes");
    assert!(!clear_session_project(&key), "second clear is no-op");
    let meta2 = read_session_meta_full(&key).expect("meta survives");
    assert!(meta2.project_id.is_none() && meta2.project_path.is_none());
    delete_chat_log(&key);
}

/// 未读标记：mark 累加 → clear true → 再 clear false → 无 meta false。
#[test]
fn undelivered_marker_accumulate_and_clear() {
    let key = cov_key("undelivered");
    delete_chat_log(&key);
    assert!(!clear_undelivered_replies(&key), "no meta → false");

    mark_undelivered_reply(&key);
    mark_undelivered_reply(&key);
    let meta = read_session_meta_full(&key).expect("meta");
    assert_eq!(meta.undelivered, 2);

    assert!(clear_undelivered_replies(&key));
    assert!(!clear_undelivered_replies(&key), "already zero → false");
    delete_chat_log(&key);
}

// ---------------------------------------------------------------------------
// wave5c：copy 前缀成功路径 / read_chat_log_async / 嵌套日志平化迁移 /
// first_user_message 跳行臂 / undelivered 缺失与归零臂
// ---------------------------------------------------------------------------

/// copy_chat_log_prefix 成功路径：按 user 轮截取前缀写入新会话文件。
#[test]
fn copy_chat_log_prefix_copies_first_turns() {
    let src = cov_key("cpsrc");
    let dst = cov_key("cpdst");
    delete_chat_log(&src);
    delete_chat_log(&dst);
    append_chat_log(&src, "user", "q1");
    append_chat_log(&src, "assistant", "a1");
    append_chat_log(&src, "user", "q2");

    // at_turn=1 → 首轮（user+assistant）保留，第二条 user 触发 break。
    let n = copy_chat_log_prefix(&src, &dst, 1);
    assert_eq!(n, 2, "first turn = user+assistant");
    let (msgs, total, _, _) = read_chat_log(&dst, 10, None);
    assert_eq!(total, 2);
    assert_eq!(msgs[0]["content"].as_str(), Some("q1"));
    assert_eq!(msgs[1]["content"].as_str(), Some("a1"));

    delete_chat_log(&src);
    delete_chat_log(&dst);
}

/// read_chat_log_async：spawn_blocking 包装正常返回行与计数。
#[tokio::test]
async fn read_chat_log_async_returns_rows() {
    let key = cov_key("async_read");
    delete_chat_log(&key);
    append_chat_log(&key, "user", "a");
    append_chat_log(&key, "assistant", "b");
    let (msgs, total, _, _) = read_chat_log_async(&key, 10, None).await;
    assert_eq!(total, 2);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1]["content"].as_str(), Some("b"));
    delete_chat_log(&key);
}

// migrate_nested_session_logs 的平化迁移测试已迁出到独立二进制
// `tests/chat_log_migrate_flatten.rs`——lib 二进制的 singleton home 被
// 启动竞速钉在真实 ~/.nemesisbot，并发跑套件时自身残留物会让「目标已
// 存在」跳过臂抢在移动断言前触发（2026-09-25 实测闪失）。隔离手法与
// 理由见该文件头注释；勿迁回。

/// first_user_message：坏行 / 非 user / 空 user 逐个跳过，trim 后截断。
#[test]
fn first_user_message_skips_bad_and_non_user_lines() {
    let key = cov_key("fum_skip");
    delete_chat_log(&key);
    let path = log_path(&key);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        concat!(
            "not json\n",
            "{\"role\":\"assistant\",\"content\":\"hi\"}\n",
            "{\"role\":\"user\",\"content\":\"   \"}\n",
            "{\"role\":\"user\",\"content\":\"  hello world  \"}\n",
        ),
    )
    .unwrap();
    assert_eq!(
        first_user_message(&key, 100).as_deref(),
        Some("hello world")
    );
    // 截断走 char 边界。
    assert_eq!(first_user_message(&key, 5).as_deref(), Some("hello"));
    delete_chat_log(&key);
}

/// undelivered 标记：meta 缺失 → clear false；归零后再 clear → false。
#[test]
fn undelivered_absent_meta_and_zero_clear() {
    let key = cov_key("und_absent");
    delete_chat_log(&key);
    assert!(!clear_undelivered_replies(&key), "absent meta → false");
    mark_undelivered_reply(&key);
    assert!(clear_undelivered_replies(&key), "nonzero → cleared");
    assert!(!clear_undelivered_replies(&key), "already zero → false");
    delete_chat_log(&key);
}
