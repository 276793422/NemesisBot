//! T6（扩展计划 D5）：chat_log 三个写路径的事件账本记账集成测试。
//! （沿用 s9_tests 的真路径 + 唯一键 + 清理形态——不触碰全局路径单例。）

use super::*;
use crate::event_ledger::{LedgerOp, LedgerRow, ledger_path, ledger_record, ledger_verify};

fn key(tag: &str) -> String {
    format!("test:t6:{}_{}", tag, std::process::id())
}

fn cleanup(k: &str) {
    delete_chat_log(k);
    let _ = std::fs::remove_file(ledger_path(k));
}

/// append 记账：append_chat_log 后账本存在、verify 绿、正文不入账（sha-only）。
#[test]
fn append_chat_log_writes_ledger_entry() {
    let k = key("append");
    cleanup(&k);
    append_chat_log(&k, "user", "T6 正文不应出现在账本里");

    let path = ledger_path(&k);
    assert!(path.exists(), "append 必须落一条账本");
    let stats = ledger_verify(&path).expect("账本链必须可验证");
    assert_eq!(stats.events, 1);
    assert_eq!(stats.appends, 1);

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("T6 正文不应出现在账本里"),
        "append 行 sha-only，正文不得入账"
    );
    cleanup(&k);
}

/// truncate 记账：被删行全文入账、可找回原文；保留行不入账。
#[test]
fn truncate_chat_log_records_deleted_rows_full_text() {
    let k = key("truncate");
    cleanup(&k);
    append_chat_log(&k, "user", "保留的提问");
    append_chat_log(&k, "assistant", "保留的回复");
    append_chat_log(&k, "user", "被 rewind 删掉的段落");

    let (all, total, _, _) = read_chat_log(&k, 100, None);
    assert_eq!(total, 3);
    let n = truncate_chat_log_rows(&k, &all[..2]);
    assert_eq!(n, 2);

    let path = ledger_path(&k);
    let stats = ledger_verify(&path).expect("truncate 后账本链必须可验证");
    assert_eq!(stats.appends, 3, "三次 append 各一条");
    assert_eq!(stats.truncates, 1);

    // 被删行全文可从账本找回。
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("被 rewind 删掉的段落"), "被删行原文必须入账");
    assert!(
        !text.contains("保留的提问"),
        "保留行不是被删行，不入 truncate 账"
    );
    // 恢复语义：账本 truncate 行的 content = 被删行**原始 jsonl 整行**（sha
    // 对整行计），据此可逐字节找回被删行。
    let last_line = text.lines().last().unwrap();
    let entry: serde_json::Value = serde_json::from_str(last_line).unwrap();
    assert_eq!(entry["op"], "truncate");
    let restored: &str = entry["rows"][0]["content"].as_str().unwrap();
    assert!(
        restored.contains("被 rewind 删掉的段落"),
        "被删行原文必须可从 content 找回，实际：{restored}"
    );
    assert_eq!(entry["rows"][0]["role"], "user");
    assert_eq!(
        entry["rows"][0]["content_sha256"],
        crate::event_ledger::sha256_hex(restored.as_bytes()),
        "sha 与 content 整行一致"
    );
    let restored_row: Value = serde_json::from_str(restored).unwrap();
    assert_eq!(restored_row["content"], "被 rewind 删掉的段落");
    cleanup(&k);
}

/// fork 记账：write_chat_log_rows 后新会话账本有 fork 事件（sha-only）。
#[test]
fn fork_rows_record_fork_event() {
    let k = key("fork");
    cleanup(&k);
    let rows = vec![
        serde_json::json!({"role": "user", "content": "fork 前缀提问", "timestamp": "T1"}),
        serde_json::json!({"role": "assistant", "content": "fork 前缀回复", "timestamp": "T2"}),
    ];
    let n = write_chat_log_rows(&k, &rows);
    assert_eq!(n, 2);

    let path = ledger_path(&k);
    let stats = ledger_verify(&path).expect("fork 账本链必须可验证");
    assert_eq!(stats.forks, 1);
    assert_eq!(stats.rows_total, 2);

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("fork 前缀提问"),
        "fork 行 sha-only，正文不得入账"
    );
    cleanup(&k);
}

/// clear 记账：清空 = 全删，被清行全文入账（与 rewind 同一恢复保障）。
#[test]
fn clear_chat_log_records_all_deleted_rows() {
    let k = key("clear");
    cleanup(&k);
    append_chat_log(&k, "user", "清空前的问题一");
    append_chat_log(&k, "assistant", "清空前的回答一");

    clear_chat_log(&k);
    assert!(!chat_log_exists(&k) || std::fs::read_to_string(log_path(&k)).unwrap() == "");

    let path = ledger_path(&k);
    let stats = ledger_verify(&path).expect("clear 后账本链必须可验证");
    assert_eq!(stats.appends, 2);
    assert_eq!(stats.truncates, 1);

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("清空前的问题一") && text.contains("清空前的回答一"),
        "被清行全文必须入账"
    );
    cleanup(&k);
}

/// ledger_record 显式入口的 best-effort 语义：坏路径只 warn 不 panic。
#[test]
fn ledger_record_best_effort_on_bad_path() {
    // 跨平台坏路径：父路径组件是一个**文件**——create_dir_all 在两平台都
    // 必败（Not a directory）。不用盘符形态（"Z:/..." 在 Linux 是合法相对
    // 目录名，会被真建出来 → 断言假红）。
    let dir = tempfile::tempdir().expect("tempdir");
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"x").expect("blocker file");
    let bad = blocker.join("child").join("t6.jsonl");
    assert!(crate::event_ledger::record_entry_at(&bad, LedgerOp::Append, "t", vec![]).is_err());
    // ledger_record（warn 吞错）不 panic。
    ledger_record(
        "test:t6:badpath",
        LedgerOp::Append,
        "t",
        vec![LedgerRow {
            role: "user".into(),
            content_sha256: "x".into(),
            content: None,
        }],
    );
    cleanup("test:t6:badpath");
}
