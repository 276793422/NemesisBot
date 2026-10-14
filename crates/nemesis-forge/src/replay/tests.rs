//! replay 单测：WAL 失败提取（二轮复查根修——工具失败 = finished+ok:false）。

use super::*;
use std::path::Path;

/// 构造一条 WAL 行（字段名与 nemesis-agent::turn_wal::WalRecord 对齐）。
fn wal_line(session: &str, tool: &str, state: &str, ok: Option<bool>, ts: u64) -> String {
    let ok_json = match ok {
        Some(b) => b.to_string(),
        None => "null".to_string(),
    };
    format!(
        r#"{{"ts_ms":{ts},"session_key":"{session}","channel":"web","chat_id":"c1","user":"修复登录 bug","call_id":"call-{ts}","tool":"{tool}","args_preview":"{{}}","state":"{state}","ok":{ok_json}}}"#
    )
}

fn write_wal(dir: &Path, lines: &[String]) {
    let wal = dir.join("logs").join("turn_wal");
    std::fs::create_dir_all(&wal).unwrap();
    std::fs::write(wal.join("wal.jsonl"), lines.join("\n")).unwrap();
}

#[test]
fn extract_picks_up_finished_tool_failures() {
    let dir = tempfile::tempdir().unwrap();
    write_wal(
        dir.path(),
        &[
            wal_line("sess-a", "write_file", "started", None, 100),
            wal_line("sess-a", "write_file", "finished", Some(false), 200),
            wal_line("sess-a", "read_file", "finished", Some(true), 300),
        ],
    );
    let out = extract_failures(dir.path()).unwrap();
    assert_eq!(out.len(), 1, "只提取 finished+ok:false，成功调用不入选");
    assert_eq!(out[0].tool, "write_file");
    assert_eq!(out[0].state, "error", "失败工具调用归一显示 error");
    assert_eq!(out[0].user, "修复登录 bug");
}

#[test]
fn extract_keeps_interrupted_and_dedups_by_session_tool() {
    let dir = tempfile::tempdir().unwrap();
    write_wal(
        dir.path(),
        &[
            wal_line("sess-a", "exec", "finished", Some(false), 100),
            wal_line("sess-a", "exec", "interrupted", None, 500),
            wal_line("sess-b", "exec", "finished", Some(false), 200),
        ],
    );
    let out = extract_failures(dir.path()).unwrap();
    assert_eq!(
        out.len(),
        2,
        "同 session+tool 去重保最新，跨 session 不合并"
    );
    // 时间倒序。
    assert_eq!(out[0].state, "interrupted");
    assert_eq!(out[0].ts_ms, 500);
    assert_eq!(out[1].state, "error");
}

#[test]
fn extract_empty_or_missing_wal_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(extract_failures(dir.path()).unwrap().is_empty());
    write_wal(dir.path(), &[wal_line("s", "t", "admitted", None, 1)]);
    assert!(extract_failures(dir.path()).unwrap().is_empty());
}

#[test]
fn to_test_case_uses_user_text_and_normalizes_name() {
    let rec = FailureRecord {
        session_key: "主会话/长度超限的会话键示例".to_string(),
        tool: "write_file".to_string(),
        user: "重构登录模块".to_string(),
        state: "error".to_string(),
        ts_ms: 1,
    };
    let case = to_test_case(&rec);
    // 会话键 sanitize 成 ASCII 安全段（中文/斜杠 → 下划线），工具名保留。
    assert!(case.name.starts_with("replay-_"), "{}", case.name);
    assert!(case.name.ends_with("-write_file"), "{}", case.name);
    assert!(
        case.name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "{}",
        case.name
    );
    assert_eq!(case.turns[0].user, "重构登录模块");
    // user 为空回落占位任务文本（含状态归一词）。
    let empty = FailureRecord {
        user: String::new(),
        ..rec
    };
    let case2 = to_test_case(&empty);
    assert!(
        case2.turns[0].user.contains("状态 error"),
        "{:?}",
        case2.turns
    );
}
