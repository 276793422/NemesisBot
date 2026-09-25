//! T6（扩展计划 D5）：会话事件账本单测。
//! （账本写验全走显式路径——不触碰 default_path_manager 全局单例。）

use super::*;
use std::path::PathBuf;

fn temp_ledger(tag: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(format!("{tag}.jsonl"));
    (dir, path)
}

fn chat_row(role: &str, content: &str) -> serde_json::Value {
    serde_json::json!({ "role": role, "content": content, "timestamp": "2026-09-24T00:00:00+08:00" })
}

/// append/truncate/fork 三种 op 全链可验证，统计准确。
#[test]
fn record_and_verify_full_chain() {
    let (_d, path) = temp_ledger("chain");

    let r1 = chat_row("user", "帮我改 bug");
    let r2 = chat_row("assistant", "已修复");
    record_entry_at(
        &path,
        LedgerOp::Append,
        "chat_log",
        vec![LedgerRow::from_chat_row(&r1, false)],
    )
    .unwrap();
    record_entry_at(
        &path,
        LedgerOp::Append,
        "chat_log",
        vec![LedgerRow::from_chat_row(&r2, false)],
    )
    .unwrap();

    // truncate：被删行全文入账
    let deleted = chat_row("user", "这段会被 rewind 删掉");
    record_entry_at(
        &path,
        LedgerOp::Truncate,
        "chat_log",
        vec![LedgerRow::from_chat_row(&deleted, true)],
    )
    .unwrap();

    // fork：sha-only
    let forked = chat_row("user", "fork 前缀");
    record_entry_at(
        &path,
        LedgerOp::Fork,
        "chat_log",
        vec![LedgerRow::from_chat_row(&forked, false)],
    )
    .unwrap();

    let stats = ledger_verify(&path).expect("全链必须验证通过");
    assert_eq!(stats.events, 4);
    assert_eq!(stats.appends, 2);
    assert_eq!(stats.truncates, 1);
    assert_eq!(stats.forks, 1);
    assert_eq!(stats.rows_total, 4);
}

/// 篡改任何一行 → 该行起 hash 断裂，verify 红。
#[test]
fn tampered_line_fails_verify() {
    let (_d, path) = temp_ledger("tamper");

    let r1 = chat_row("user", "原始内容");
    record_entry_at(
        &path,
        LedgerOp::Append,
        "chat_log",
        vec![LedgerRow::from_chat_row(&r1, false)],
    )
    .unwrap();
    record_entry_at(&path, LedgerOp::Append, "chat_log", vec![]).unwrap();

    // 篡改第一行正文相关字段（改 actor——append 行的 sha 不含它，但 hash 链含）。
    let text = std::fs::read_to_string(&path).unwrap();
    let tampered = text.replace("\"actor\":\"chat_log\"", "\"actor\":\"forger\"");
    assert_ne!(text, tampered, "替换必须命中");
    std::fs::write(&path, tampered).unwrap();

    let err = ledger_verify(&path).expect_err("篡改必须红");
    assert!(err.contains("line 1"), "应定位到第 1 行，实际：{err}");
}

/// 删除中间一行 → 后继行 prev_hash 断裂。
#[test]
fn deleted_middle_line_fails_verify() {
    let (_d, path) = temp_ledger("delete");

    record_entry_at(&path, LedgerOp::Append, "chat_log", vec![]).unwrap();
    record_entry_at(&path, LedgerOp::Append, "chat_log", vec![]).unwrap();
    record_entry_at(&path, LedgerOp::Append, "chat_log", vec![]).unwrap();

    let lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(lines.len(), 3);
    // 删第 2 行：第 3 行的 seq=2 但链位置=1 → seq 闸先红（prev_hash 同断）。
    std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
    let err = ledger_verify(&path).expect_err("删行必须红");
    assert!(
        err.contains("seq 断裂") || err.contains("prev_hash"),
        "应报链断裂，实际：{err}"
    );
}

/// truncate 行的全文可找回（被删行原文 = 账本 content 字段）。
#[test]
fn truncated_rows_recoverable_from_ledger() {
    let (_d, path) = temp_ledger("recover");

    let deleted1 = chat_row("user", "rewind 删掉的提问");
    let deleted2 = chat_row("assistant", "rewind 删掉的回复");
    record_entry_at(
        &path,
        LedgerOp::Truncate,
        "chat_log",
        vec![
            LedgerRow::from_chat_row(&deleted1, true),
            LedgerRow::from_chat_row(&deleted2, true),
        ],
    )
    .unwrap();

    // 从账本找回被删行原文。
    let text = std::fs::read_to_string(&path).unwrap();
    let entry: LedgerEntry = serde_json::from_str(text.trim()).unwrap();
    let contents: Vec<&str> = entry
        .body
        .rows
        .iter()
        .filter_map(|r| r.content.as_deref())
        .collect();
    assert_eq!(contents, vec!["rewind 删掉的提问", "rewind 删掉的回复"]);
    assert!(ledger_verify(&path).is_ok());
}

/// 行内正文与 sha 矛盾（截断行被手改）→ verify 红。
#[test]
fn content_sha_mismatch_fails_verify() {
    let (_d, path) = temp_ledger("shamismatch");

    let deleted = chat_row("user", "原文");
    record_entry_at(
        &path,
        LedgerOp::Truncate,
        "chat_log",
        vec![LedgerRow::from_chat_row(&deleted, true)],
    )
    .unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    let tampered = text.replace("原文", "改过的假原文");
    std::fs::write(&path, tampered).unwrap();
    // 截断行被手改 → 正文字节变 → 链 hash 闸先红；sha 闸是 hash 巧合通过
    // 时的第二道（行内正文 vs content_sha256）。
    let err = ledger_verify(&path).expect_err("正文与 sha 矛盾必须红");
    assert!(
        err.contains("sha") || err.contains("hash 不匹配"),
        "应报 sha 不一致或链 hash 断裂，实际：{err}"
    );
}

/// append 行不带正文（内容策略：append sha-only）。
#[test]
fn append_rows_are_sha_only() {
    let (_d, path) = temp_ledger("appendonly");

    let r = chat_row("user", "正文不应入账本");
    record_entry_at(
        &path,
        LedgerOp::Append,
        "chat_log",
        vec![LedgerRow::from_chat_row(&r, false)],
    )
    .unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("正文不应入账本"), "append 行不得携带正文");
    let entry: LedgerEntry = serde_json::from_str(text.trim()).unwrap();
    assert!(entry.body.rows[0].content.is_none());
    assert_eq!(
        entry.body.rows[0].content_sha256,
        sha256_hex("正文不应入账本".as_bytes())
    );
}

/// 创世行 prev_hash = 全 0；空文件 verify = 全零统计不报错。
#[test]
fn genesis_and_empty_file() {
    let (_d, path) = temp_ledger("genesis");

    record_entry_at(&path, LedgerOp::Append, "chat_log", vec![]).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let entry: LedgerEntry = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(entry.prev_hash, GENESIS_PREV_HASH);
    assert_eq!(entry.body.seq, 0);

    let empty = _d.path().join("empty.jsonl");
    std::fs::write(&empty, "").unwrap();
    let stats = ledger_verify(&empty).unwrap();
    assert_eq!(stats.events, 0);
}
