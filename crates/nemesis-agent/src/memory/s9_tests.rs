//! S9 覆盖率批次：memory.rs 剩余未覆盖行。
//!
//! - 102/186/216/238：tracing 宏参数表达式行（debug!/info! 的字段在无
//!   subscriber 时不求值）→ `capture_logs()` 装上 thread-local subscriber。
//! - 111：summarize 的 no-system-prompt 分支（turns[0] 非 system 且被保留）。
//! - 220/245：write_long_term / append_today 的 if-let 块收尾。

use super::*;
use crate::test_support::capture_logs;

fn turn(role: &str, content: &str) -> ConversationTurn {
    ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: "2026-04-29T12:00:00Z".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }
}

/// summarize 在无 system prompt 时也能工作：debug! 参数行被求值（102），
/// 且保留的 turns[0] 非 system → 走 111 的 no-op 分支。
#[test]
fn summarize_without_system_prompt_hits_noop_arm_and_logs() {
    let _logs = capture_logs();
    let mut mem = ConversationMemory::new(MemoryConfig {
        max_tokens: 10_000,
        keep_tokens: 10, // tiny target → drops almost everything
    });
    // 无 system：全部 user 轮
    for i in 0..8 {
        mem.add(turn("user", &format!("message number {} with padding text", i)));
    }
    let removed = mem.summarize();
    assert!(removed > 0, "must remove some turns, removed={}", removed);
    // 保留至少 1 轮（keep_from.max(1)）
    assert!(!mem.get_context().is_empty());
}

/// MemoryStore::new 的 info! 参数行（186）。
#[test]
fn memory_store_new_logs_info_line() {
    let _logs = capture_logs();
    let dir = std::env::temp_dir().join(format!("nemesis_memstore_new_{}_{}", std::process::id(), line!()));
    let _ = std::fs::remove_dir_all(&dir);
    let _store = MemoryStore::new(dir.to_string_lossy().as_ref());
    let _ = std::fs::remove_dir_all(&dir);
}

/// write_long_term 的 debug! 参数行（216）+ if-let 收尾（220）。
#[test]
fn memory_store_write_long_term_logs_and_roundtrips() {
    let _logs = capture_logs();
    let dir = std::env::temp_dir().join(format!("nemesis_memstore_lt_{}_{}", std::process::id(), line!()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = MemoryStore::new(dir.to_string_lossy().as_ref());
    store.write_long_term("长期记忆内容").unwrap();
    assert_eq!(store.read_long_term(), "长期记忆内容");
    let _ = std::fs::remove_dir_all(&dir);
}

/// append_today 的 debug! 参数行（238）+ if-let 收尾（245）：首写建头、再写追加。
#[test]
fn memory_store_append_today_creates_then_appends() {
    let _logs = capture_logs();
    let dir = std::env::temp_dir().join(format!("nemesis_memstore_today_{}_{}", std::process::id(), line!()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = MemoryStore::new(dir.to_string_lossy().as_ref());
    store.append_today("first note").unwrap();
    let first = store.read_today();
    assert!(first.contains("first note"), "first read: {}", first);
    store.append_today("second note").unwrap();
    let second = store.read_today();
    assert!(second.contains("first note") && second.contains("second note"));
    let _ = std::fs::remove_dir_all(&dir);
}
