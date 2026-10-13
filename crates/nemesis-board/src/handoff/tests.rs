//! U-A handoff 单元测试（临时 store + 临时目录，零网络零 LLM）。

use super::*;
use crate::assignment::Actor;
use crate::models::NewIssue;
use crate::store::BoardStore;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-handofftest-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn spec() -> HandoffSpec {
    HandoffSpec {
        stop_kind: "fail_streak",
        suggested_action: "manual_check",
        hint: "同因熔断：先修 worker 环境再复评".into(),
        resume_entry: "review.retrigger（number=NB-1）".into(),
        environment: serde_json::json!({ "last_worker": "node-b" }),
        attempted: vec![serde_json::json!({ "action": "redispatch", "result": "第 3 轮 FAIL" })],
    }
}

#[test]
fn test_write_read_roundtrip_preserves_fields() {
    let ws = temp_dir("roundtrip");
    let rec = spec();
    let chain = vec![serde_json::json!({
        "at": 1_700_000_000i64,
        "decision": "redispatch",
        "verdict": "FAIL",
        "details": { "decision": "redispatch", "verdict": "FAIL", "round": 1 },
    })];
    let path = write_handoff(&ws, "NB-7", 7, &rec, chain.clone()).expect("write");
    assert_eq!(path, handoff_path(&ws, "NB-7"));
    assert!(path.file_name().unwrap() == "NB-7.json");

    let doc = read_handoff(&ws, "NB-7").expect("read");
    assert_eq!(doc["schema"], 1);
    assert_eq!(doc["issue"], "NB-7");
    assert_eq!(doc["issue_id"], 7);
    assert_eq!(doc["stop_kind"], "fail_streak");
    assert_eq!(doc["suggested_action"], "manual_check");
    assert_eq!(doc["hint"], "同因熔断：先修 worker 环境再复评");
    assert_eq!(doc["resume_entry"], "review.retrigger（number=NB-1）");
    assert_eq!(doc["environment"]["last_worker"], "node-b");
    assert_eq!(doc["attempted"].as_array().unwrap().len(), 1);
    assert_eq!(doc["verdict_chain"], serde_json::Value::Array(chain));
    assert!(doc["stopped_at"].as_str().unwrap().contains("T"));
    // 覆盖写：二次停车最新态胜出（verdict_chain 由调用方传全程）。
    let rec2 = HandoffSpec {
        stop_kind: "unsure_streak",
        ..spec()
    };
    write_handoff(&ws, "NB-7", 7, &rec2, Vec::new()).expect("overwrite");
    let doc2 = read_handoff(&ws, "NB-7").expect("read2");
    assert_eq!(doc2["stop_kind"], "unsure_streak");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn test_read_missing_handoff_is_honest_error() {
    let ws = temp_dir("missing");
    assert!(read_handoff(&ws, "NB-404").is_err());
    let _ = std::fs::remove_dir_all(&ws);
}

/// verdict_chain 提取：auto_decide 活动行全程链（决策词/verdict 从
/// details JSON 解出）；非 auto_decide 活动行与评论不混入。
#[test]
fn test_verdict_chain_from_timeline_aggregate() {
    let dir = temp_dir("chain");
    let store = BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    let issue = store
        .create_issue(NewIssue {
            title: "链".into(),
            creator: Actor::admin("tester"),
            ..NewIssue::default()
        })
        .unwrap();
    store
        .add_activity(
            issue.id,
            &Actor::admin("board"),
            "auto_decide",
            Some(r#"{"decision":"redispatch","verdict":"FAIL","round":1}"#),
        )
        .unwrap();
    store
        .add_activity(
            issue.id,
            &Actor::admin("tester"),
            "status_changed",
            Some("todo→done"),
        )
        .unwrap();
    store
        .add_activity(
            issue.id,
            &Actor::admin("board"),
            "auto_decide",
            Some(r#"{"decision":"escalate_human","verdict":"FAIL","reason":"fail_signature_streak"}"#),
        )
        .unwrap();

    let chain = verdict_chain_from(&store, issue.id);
    assert_eq!(chain.len(), 2, "chain: {chain:?}");
    assert_eq!(chain[0]["decision"], "redispatch");
    assert_eq!(chain[0]["verdict"], "FAIL");
    assert_eq!(chain[1]["decision"], "escalate_human");
    assert_eq!(chain[1]["details"]["reason"], "fail_signature_streak");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_write_failure_on_file_workspace_path() {
    // workspace 指向文件 → create_dir_all 失败 → 诚实 Err（调用方降级）。
    let dir = temp_dir("badws");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    assert!(write_handoff(&blocker, "NB-1", 1, &spec(), Vec::new()).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
