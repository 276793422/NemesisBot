//! 快照复活封堵 + rewind 分支摘要撤除（2026-11-24）的 chat_log 面测试。
//!
//! 覆盖：`session_context_alive` 三态（缺失 / 空文件 / 有行）——续行投递
//! 闸的盘上判据；`clear_session_branch_summary` purge（写→清→读 None）。
//! unique key + 清理（本目录测试惯例——走进程单例 home，不重定向）。

use super::*;

fn unique_key(tag: &str) -> String {
    format!(
        "test:contgate:{tag}:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn cleanup(key: &str) {
    delete_chat_log(key);
    let _ = std::fs::remove_file(meta_path(key));
}

/// jsonl 缺失 = 会话从未存在或已被删除（session delete 删文件）→ 死。
#[test]
fn context_alive_missing_file_is_false() {
    let key = unique_key("missing");
    assert!(!session_context_alive(&key));
}

/// jsonl 存在但零行 = chat.clear（truncate 落空文件）→ 同样判死。
#[test]
fn context_alive_empty_file_is_false() {
    let key = unique_key("empty");
    append_chat_log(&key, "user", "hello");
    clear_chat_log(&key).expect("clear 落空文件");
    assert!(
        log_path(&key).exists(),
        "clear 后文件应存在（truncate 非删除）"
    );
    assert!(!session_context_alive(&key));
    cleanup(&key);
}

/// jsonl 有 ≥1 行 = 活上下文（快照保存时 user 行恒已在轮首落盘）。
#[test]
fn context_alive_with_row_is_true() {
    let key = unique_key("alive");
    append_chat_log(&key, "user", "hello");
    assert!(session_context_alive(&key));
    cleanup(&key);
}

/// rewind purge：写摘要 → clear → meta 读回 None；再写可恢复（幂等
/// upsert 语义不受 purge 影响）。
#[test]
fn clear_branch_summary_purges_sidecar() {
    let key = unique_key("purge");
    write_session_branch_summary(&key, "被删轮的结构化前情提要");
    assert!(
        read_session_meta_full(&key)
            .and_then(|m| m.branch_summary)
            .is_some()
    );
    clear_session_branch_summary(&key);
    assert!(
        read_session_meta_full(&key)
            .and_then(|m| m.branch_summary)
            .is_none()
    );
    // purge 后再写照常生效（后续 fork 语义不受本次 purge 影响）。
    write_session_branch_summary(&key, "再次生成");
    assert!(
        read_session_meta_full(&key)
            .and_then(|m| m.branch_summary)
            .is_some()
    );
    cleanup(&key);
}

/// 空白摘要写不落盘（write 侧既有 trim 语义）+ purge 对无 meta 会话
/// 安全（upsert 会创建空基线 meta）。
#[test]
fn branch_summary_empty_write_noop_and_purge_creates_baseline() {
    let key = unique_key("noop");
    write_session_branch_summary(&key, "   ");
    assert!(
        read_session_meta_full(&key)
            .and_then(|m| m.branch_summary)
            .is_none()
    );
    // 无 meta 的会话直接 purge：upsert 创建空基线 meta，branch_summary
    // 恒 None——不炸不产生幽灵摘要。
    let key2 = unique_key("purge-no-meta");
    clear_session_branch_summary(&key2);
    assert!(
        read_session_meta_full(&key2)
            .and_then(|m| m.branch_summary)
            .is_none()
    );
    cleanup(&key);
    cleanup(&key2);
}
