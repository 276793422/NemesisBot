//! B11（2026-10-18 snooze）：scan_session_logs 的暂停标记回填——sidecar
//! meta 带 `snoozed_until` 的条目补 `snoozedUntil`（+ `snoozeNote` 跟随）；
//! 无标记 meta / 无 meta 文件零增量（不加字段）。
//!
//! 夹具：workspace=tempdir，`logs/sessions/` 下手工落 jsonl + meta.json
//! （与 chat_log.rs 写盘同形），不依赖全局 path manager（e4_lineage_tests
//! 同款）。

use super::*;

/// 在 workspace 的 session_logs 下造一个会话：一行 user jsonl + 可选 meta。
fn seed_session(ws: &Path, stem: &str, first: &str, meta_json: Option<&str>) {
    let dir = session_log_dir(ws.to_str().unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let row = serde_json::json!({
        "role": "user",
        "content": first,
        "timestamp": "2026-10-18T00:00:00+08:00"
    });
    std::fs::write(
        dir.join(format!("{stem}.jsonl")),
        serde_json::to_string(&row).unwrap() + "\n",
    )
    .unwrap();
    if let Some(m) = meta_json {
        std::fs::write(dir.join(format!("{stem}.meta.json")), m).unwrap();
    }
}

#[test]
fn scan_backfills_snoozed_until_and_note() {
    let ws = tempfile::tempdir().unwrap();
    let meta = r#"{"title":"暂停中的会话","snoozed_until":"2026-10-19T09:00:00+08:00","snooze_note":"等部署窗口"}"#;
    seed_session(ws.path(), "agent_main_session_sz1", "first", Some(meta));
    // 无标记 meta（暂停已摘除）与无 meta 文件：两种缺省形态。
    seed_session(
        ws.path(),
        "agent_main_session_sz2",
        "cleared",
        Some(r#"{"title":"已恢复"}"#),
    );
    seed_session(ws.path(), "agent_main_session_sz3", "bare", None);

    let sessions = scan_session_logs(ws.path().to_str().unwrap());
    let snoozed = sessions
        .iter()
        .find(|s| s["id"] == "agent_main_session_sz1")
        .expect("snoozed 会话在列表中");
    assert_eq!(snoozed["snoozedUntil"], "2026-10-19T09:00:00+08:00");
    assert_eq!(snoozed["snoozeNote"], "等部署窗口");

    let cleared = sessions
        .iter()
        .find(|s| s["id"] == "agent_main_session_sz2")
        .expect("cleared 会话在列表中");
    assert!(cleared.get("snoozedUntil").is_none(), "无标记不加字段");
    assert!(cleared.get("snoozeNote").is_none());

    let bare = sessions
        .iter()
        .find(|s| s["id"] == "agent_main_session_sz3")
        .expect("bare 会话在列表中");
    assert!(bare.get("snoozedUntil").is_none(), "无 meta 不加字段");
}

#[test]
fn scan_backfills_snoozed_until_without_note() {
    let ws = tempfile::tempdir().unwrap();
    // note=None 的写盘形态：snooze_note 键不落盘（skip_serializing_if）。
    seed_session(
        ws.path(),
        "agent_main_session_sz4",
        "no note",
        Some(r#"{"snoozed_until":"2099-01-01T00:00:00+08:00"}"#),
    );
    let sessions = scan_session_logs(ws.path().to_str().unwrap());
    let hit = sessions
        .iter()
        .find(|s| s["id"] == "agent_main_session_sz4")
        .expect("会话在列表中");
    assert_eq!(hit["snoozedUntil"], "2099-01-01T00:00:00+08:00");
    assert!(hit.get("snoozeNote").is_none(), "无 note 不加 note 字段");
}
