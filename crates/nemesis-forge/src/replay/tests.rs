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
fn extract_scans_rotated_wal_old_generation() {
    // 轮换代扫描（四路评审 C-F3 根修）：wal.jsonl 轮换为 wal.jsonl.old 后，
    // 上一代的失败记录仍要被提取（当前代只有新失败时不丢上一代尾部）。
    let dir = tempfile::tempdir().unwrap();
    write_wal(
        dir.path(),
        &[wal_line("sess-new", "exec", "finished", Some(true), 900)],
    );
    let wal = dir.path().join("logs").join("turn_wal");
    std::fs::write(
        wal.join("wal.jsonl.old"),
        wal_line("sess-old", "write_file", "finished", Some(false), 100),
    )
    .unwrap();
    let out = extract_failures(dir.path()).unwrap();
    assert_eq!(out.len(), 1, "轮换代的失败必须入选");
    assert_eq!(out[0].tool, "write_file");
}

#[test]
fn confirm_writes_confirmed_before_pending_so_crash_never_loses_cases() {
    // 写序回归锁（四路评审 C-F1 根修）：confirmed 写失败（此处以目录占位
    // 模拟中断点）时 pending 必须原样——用例不被销毁；写序颠倒的旧实现会
    // 先截 pending 再写 confirmed，中断即整批丢失。
    let dir = tempfile::tempdir().unwrap();
    let case = TestCase {
        name: "replay-case-a".into(),
        turns: vec![TestTurn {
            user: "修复登录 bug".into(),
        }],
        fixtures: vec![],
        expect: TestExpectations::default(),
        judge: None,
    };
    let (target, n) = ingest(dir.path(), &[case], false).unwrap();
    assert_eq!(target, "pending");
    assert_eq!(n, 1);
    // confirmed.json 预置为目录 → write_cases(confirmed) 必失败（中断点）。
    let confirmed_path = dir
        .path()
        .join("forge")
        .join("replay")
        .join("confirmed.json");
    std::fs::create_dir_all(&confirmed_path).unwrap();
    assert!(
        confirm(dir.path(), None).is_err(),
        "confirmed 写失败必须报错"
    );
    // pending 原样：用例未丢。（confirmed.json 已被目录毒化，list_full 会
    // 诚实 Err——直接读 pending 文件内容验证。）
    let pending_raw =
        std::fs::read_to_string(dir.path().join("forge").join("replay").join("pending.json"))
            .unwrap();
    let pending: Vec<TestCase> = serde_json::from_str(&pending_raw).unwrap();
    assert_eq!(pending.len(), 1, "中断点之后 pending 必须原样保留用例");
}

#[test]
fn corrupt_zone_files_error_instead_of_silent_empty() {
    // 损坏区文件（四路评审 C-F2 根修）：静默当空会让后续 confirm/ingest
    // 以空基线回写销毁整区——必须 Err。
    let dir = tempfile::tempdir().unwrap();
    let replay = dir.path().join("forge").join("replay");
    std::fs::create_dir_all(&replay).unwrap();
    std::fs::write(replay.join("confirmed.json"), "{not json").unwrap();
    std::fs::write(replay.join("pending.json"), "[]").unwrap();
    assert!(confirmed_cases(dir.path()).is_err());
    assert!(list_full(dir.path()).is_err());
    assert!(list(dir.path()).is_err());
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
