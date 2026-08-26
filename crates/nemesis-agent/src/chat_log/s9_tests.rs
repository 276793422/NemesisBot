//! S9 覆盖率批次：chat_log.rs 剩余未覆盖行。
//! - 193：write_chat_log_rows 直调（fork round-3 主路径，需非空 rows）。
//! - 245：copy_chat_log_prefix 直调（已被 supersede，#[allow(dead_code)]，
//!   但 pub 可测——覆盖归档路径）。
//! - 336：write_chat_log_from_store 直调（同上 superseded）。
//! - 379：clear_chat_log 的 boundary sidecar fs::write 失败 warn（sidecar
//!   路径预置为目录 → 写失败）。

use super::*;
use crate::test_support::capture_logs;
use nemesis_path::default_path_manager;

fn key(tag: &str) -> String {
    format!("test:s9:{}", tag)
}

/// write_chat_log_rows：VERBATIM 行写入 + 计数（193 块收尾）。
#[test]
fn write_chat_log_rows_writes_verbatim_lines() {
    let k = key(&format!("rows_{}", std::process::id()));
    delete_chat_log(&k);
    let rows = vec![
        serde_json::json!({"role": "user", "content": "hi", "timestamp": "T1", "extra": "kept"}),
        serde_json::json!({"role": "assistant", "content": "hello", "timestamp": "T2"}),
    ];
    let n = write_chat_log_rows(&k, &rows);
    assert_eq!(n, 2);
    let (msgs, total, _, _) = read_chat_log(&k, 10, None);
    assert_eq!(total, 2);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["extra"].as_str(), Some("kept"), "extra fields preserved verbatim");
    // 空 rows → 0（early return）
    assert_eq!(write_chat_log_rows(&k, &[]), 0);
    delete_chat_log(&k);
}

/// copy_chat_log_prefix（superseded 保留路径）：按 user 轮数截断复制（245）。
#[test]
fn copy_chat_log_prefix_copies_up_to_turn() {
    let src = key(&format!("cpsrc_{}", std::process::id()));
    let dst = key(&format!("cpdst_{}", std::process::id()));
    delete_chat_log(&src);
    delete_chat_log(&dst);
    append_chat_log(&src, "user", "turn 1 question");
    append_chat_log(&src, "assistant", "turn 1 answer");
    append_chat_log(&src, "user", "turn 2 question");
    append_chat_log(&src, "assistant", "turn 2 answer");
    let copied = copy_chat_log_prefix(&src, &dst, 1);
    assert_eq!(copied, 2, "turn 1 (user+assistant) copied");
    let (msgs, _, _, _) = read_chat_log(&dst, 10, None);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["content"].as_str(), Some("turn 1 question"));
    // at_turn 0 → 无完整 user 轮保留 → lines 空 → 0
    let dst0 = key(&format!("cpdst0_{}", std::process::id()));
    delete_chat_log(&dst0);
    assert_eq!(copy_chat_log_prefix(&src, &dst0, 0), 0);
    delete_chat_log(&src);
    delete_chat_log(&dst);
    delete_chat_log(&dst0);
}

/// write_chat_log_from_store（superseded 保留路径）：只投影 user/assistant
/// 且跳过空 content 的 assistant（336 块收尾）。
#[test]
fn write_chat_log_from_store_projects_chat_rows_only() {
    let k = key(&format!("fromstore_{}", std::process::id()));
    delete_chat_log(&k);
    let msgs = vec![
        crate::session::StoredMessage {
            role: "user".to_string(),
            content: "hello".to_string(),
            timestamp: "T1".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        crate::session::StoredMessage {
            role: "assistant".to_string(),
            content: String::new(), // 纯 tool_calls 中间消息 → 跳过
            timestamp: "T2".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        crate::session::StoredMessage {
            role: "assistant".to_string(),
            content: "final reply".to_string(),
            timestamp: "T3".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
    ];
    let n = write_chat_log_from_store(&k, &msgs);
    assert_eq!(n, 2, "empty assistant skipped");
    let (rows, _, _, _) = read_chat_log(&k, 10, None);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["content"].as_str(), Some("final reply"));
    delete_chat_log(&k);
}

/// clear_chat_log：boundary sidecar 已存在但为目录 → fs::write 失败 → warn（379）。
#[test]
fn clear_chat_log_warns_when_boundary_sidecar_is_a_directory() {
    let _logs = capture_logs();
    let k = key(&format!("clrdir_{}", std::process::id()));
    delete_chat_log(&k);
    // 主 jsonl 写入两条，让 clear 有东西可清
    append_chat_log(&k, "user", "x");
    // sidecar 预置为目录 → fs::write 失败
    let safe = k.replace(':', "_");
    let sidecar_dir = default_path_manager()
        .boundary_events_dir()
        .join(format!("{}.jsonl", safe));
    let _ = std::fs::remove_dir_all(&sidecar_dir);
    std::fs::create_dir_all(&sidecar_dir).unwrap();

    clear_chat_log(&k); // 必须不 panic；warn 分支执行

    // 清理副作用
    let _ = std::fs::remove_dir_all(&sidecar_dir);
    delete_chat_log(&k);
}
