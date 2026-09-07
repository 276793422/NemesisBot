//! E4（2026-09-05）：scan_session_logs 的 fork 血缘回填——sidecar meta 带
//! `parent`/`forked_at_turn` 的条目补 `parent`/`parentTitle`/`forkedAtTurn`
//! 三字段；pre-E4 meta（只有 title）与父会话已删的场景干净缺省（不加字段/
//! parentTitle 缺省由前端回退显示 parent key）。
//!
//! 夹具：workspace=tempdir，`logs/sessions/` 下手工落 jsonl + meta.json
//! （与 chat_log.rs 写盘同形），不依赖全局 path manager。

use super::*;

/// 在 workspace 的 session_logs 下造一个会话：一行 user jsonl + 可选 meta。
fn seed_session(ws: &Path, stem: &str, first: &str, meta_json: Option<&str>) {
    let dir = session_log_dir(ws.to_str().unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let row = serde_json::json!({
        "role": "user",
        "content": first,
        "timestamp": "2026-09-05T00:00:00+08:00"
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
fn scan_backfills_parent_parent_title_and_turn() {
    let ws = tempfile::tempdir().unwrap();
    // 父会话带 title；fork 子会话 meta 带血缘（fork_session 成功路径形态）。
    seed_session(
        ws.path(),
        "agent_main_session_p1",
        "parent first",
        Some(r#"{"title":"父会话"}"#),
    );
    seed_session(
        ws.path(),
        "agent_main_session_p1__fork",
        "fork first",
        Some(r#"{"title":"子会话","parent":"agent:main:session:p1","forked_at_turn":3}"#),
    );

    let sessions = scan_session_logs(ws.path().to_str().unwrap());
    let fork = sessions
        .iter()
        .find(|s| s["id"] == "agent_main_session_p1__fork")
        .expect("fork 会话在列表中");
    assert_eq!(fork["parent"], "agent:main:session:p1");
    assert_eq!(fork["parentTitle"], "父会话", "父标题就地解析");
    assert_eq!(fork["forkedAtTurn"], 3);

    let parent = sessions
        .iter()
        .find(|s| s["id"] == "agent_main_session_p1")
        .expect("父会话在列表中");
    assert!(parent.get("parent").is_none(), "非 fork 会话不加血缘字段");
    assert!(parent.get("forkedAtTurn").is_none());
}

#[test]
fn scan_legacy_meta_and_missing_parent_degrade_cleanly() {
    let ws = tempfile::tempdir().unwrap();
    // pre-E4 meta：只有 title → 不加任何血缘字段。
    seed_session(
        ws.path(),
        "agent_main_session_old",
        "old first",
        Some(r#"{"title":"old"}"#),
    );
    // 血缘指向已删父会话 → parent 有、parentTitle 缺省。
    seed_session(
        ws.path(),
        "agent_main_session_orphan",
        "orphan first",
        Some(r#"{"parent":"agent:main:session:gone","forked_at_turn":2}"#),
    );

    let sessions = scan_session_logs(ws.path().to_str().unwrap());
    let old = sessions
        .iter()
        .find(|s| s["id"] == "agent_main_session_old")
        .expect("旧会话在列表中");
    assert!(old.get("parent").is_none());
    assert!(old.get("parentTitle").is_none());
    assert!(old.get("forkedAtTurn").is_none());

    let orphan = sessions
        .iter()
        .find(|s| s["id"] == "agent_main_session_orphan")
        .expect("孤儿 fork 在列表中");
    assert_eq!(orphan["parent"], "agent:main:session:gone");
    assert!(
        orphan.get("parentTitle").is_none(),
        "父会话已删 → 标题缺省（前端回退显示 parent key）"
    );
    assert_eq!(orphan["forkedAtTurn"], 2);
}
