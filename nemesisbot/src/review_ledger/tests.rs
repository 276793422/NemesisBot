//! `review_ledger` 用例（P4-d）：评审结论入账（review 行 payload/run_id）、
//! 分支闭环两臂（auto_accept→confirm / redispatch→reject 带理由）、
//! scope 闸（pending/parent_* 不动分支）、无账本静默、dossier 行。
//! 全部走真 TaskDb（tempdir）与 `Cluster::new` deps——task_db 显式传参，
//! 与生产调用方 `deps.cluster.task_db()` 提取同形。

use super::*;
use crate::board_review::SelfcheckRegistry;
use nemesis_board::{Actor, NewIssue};
use nemesis_cluster::cluster::Cluster;
use nemesis_cluster::types::ClusterConfig;
use std::sync::Arc;

fn deps_with_issue(dir_name: &str) -> (BoardReviewDeps, nemesis_board::Issue) {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-review-ledger-{}-{dir_name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let store =
        Arc::new(nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store"));
    let issue = store
        .create_issue(NewIssue {
            title: "评审闭环夹具".to_string(),
            description: String::new(),
            priority: 2,
            creator: Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();
    let cluster = Arc::new(Cluster::new(ClusterConfig {
        node_id: "node-a".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: vec![],
        node_name: String::new(),
    }));
    let deps = BoardReviewDeps {
        store,
        workspace: dir.clone(),
        home: dir,
        moderator_loop: Arc::new(std::sync::OnceLock::new()),
        cluster,
        estop: Arc::new(nemesis_agent::estop::EstopState::new()),
        estop_parked: Arc::new(std::sync::Mutex::new(Vec::new())),
        selfcheck: SelfcheckRegistry::new(),
        #[cfg(feature = "security")]
        security_slot: None,
    };
    (deps, issue)
}

fn open_db(dir_name: &str) -> Arc<nemesis_taskstore::TaskDb> {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-review-ledger-db-{}-{dir_name}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    // TaskDb::open 本身回 Arc<TaskDb>（内部连接池句柄形态）。
    nemesis_taskstore::TaskDb::open(&dir.join("tasks.db")).expect("open taskdb")
}

#[test]
fn review_row_records_decision_and_run_id() {
    let (deps, issue) = deps_with_issue("row");
    let db = open_db("row");
    deps.store
        .insert_dispatch("t-1", issue.id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    review_outcome_delivery(
        &deps,
        Some(&db),
        issue.id,
        "escalate_human",
        "fail",
        &serde_json::json!({ "reasons": "差距原文" }),
    );
    let rows = db.deliveries_by_run("t-1").unwrap();
    assert_eq!(rows.len(), 1, "review 行应挂最新派发的 run_id");
    let row = &rows[0];
    assert_eq!(row.kind, "review");
    // 项 C：记账即义务登记（recorded），review 行无出站义务（notify none）。
    assert_eq!(row.status, "recorded");
    assert_eq!(row.notify_state, "none");
    assert_eq!(payload_field(row, "scope"), "issue");
    assert_eq!(payload_field(row, "decision"), "escalate_human");
    assert_eq!(payload_field(row, "task_id"), "t-1");
    assert_eq!(payload_field(row, "issue_number"), issue.number.as_str());
}

#[test]
fn auto_accept_confirms_branch_delivery() {
    let (deps, issue) = deps_with_issue("accept");
    let db = open_db("accept");
    deps.store
        .insert_dispatch("t-2", issue.id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    db.record_delivery(
        "branch",
        Some("t-2"),
        "t-2:branch",
        &serde_json::json!({ "branch": "run-t-2" }),
        false,
    )
    .unwrap();
    review_outcome_delivery(
        &deps,
        Some(&db),
        issue.id,
        "auto_accept",
        "pass",
        &serde_json::json!({}),
    );
    assert_eq!(
        db.delivery_by_key("t-2:branch").unwrap().unwrap().status,
        "confirmed",
        "auto_accept 必须确认分支交付"
    );
}

#[test]
fn redispatch_rejects_branch_with_review_reason() {
    let (deps, issue) = deps_with_issue("reject");
    let db = open_db("reject");
    deps.store
        .insert_dispatch("t-3", issue.id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    db.record_delivery(
        "branch",
        Some("t-3"),
        "t-3:branch",
        &serde_json::json!({ "branch": "run-t-3" }),
        false,
    )
    .unwrap();
    review_outcome_delivery(
        &deps,
        Some(&db),
        issue.id,
        "redispatch",
        "fail",
        &serde_json::json!({ "reasons": "- 锚点缺失" }),
    );
    let row = db.delivery_by_key("t-3:branch").unwrap().unwrap();
    assert_eq!(
        row.status, "rejected",
        "redispatch 必须拒绝该次尝试的分支交付"
    );
    let reason = row.reject_reason.expect("拒绝必须带理由");
    assert!(reason.contains("评审未通过"), "got: {reason}");
    assert!(
        reason.contains("锚点缺失"),
        "理由应带评审差距原文: {reason}"
    );
}

#[test]
fn pending_decisions_and_parent_scope_leave_branch_untouched() {
    let (deps, issue) = deps_with_issue("pending");
    let db = open_db("pending");
    deps.store
        .insert_dispatch("t-4", issue.id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    db.record_delivery(
        "branch",
        Some("t-4"),
        "t-4:branch",
        &serde_json::json!({ "branch": "run-t-4" }),
        false,
    )
    .unwrap();
    // escalate_human → pending 留人工（WSAPI deliveries.confirm/reject）。
    review_outcome_delivery(
        &deps,
        Some(&db),
        issue.id,
        "escalate_human",
        "unsure",
        &serde_json::json!({}),
    );
    assert_eq!(
        db.delivery_by_key("t-4:branch").unwrap().unwrap().status,
        "recorded",
        "escalate_human 不做分支闭环"
    );
    // parent_* scope 只记证据行，即便同 issue 有派发也不闭环。
    review_outcome_delivery(
        &deps,
        Some(&db),
        issue.id,
        "parent_auto_close",
        "pass",
        &serde_json::json!({}),
    );
    assert_eq!(
        db.delivery_by_key("t-4:branch").unwrap().unwrap().status,
        "recorded",
        "parent scope 不做分支闭环"
    );
    // 两条决策各成 review 行（每次决策是独立事件，不互斥）。
    let reviews = db
        .deliveries_recent(50)
        .unwrap()
        .iter()
        .filter(|r| r.kind == "review")
        .count();
    assert_eq!(reviews, 2);
}

#[test]
fn no_task_db_is_silent_noop() {
    let (deps, issue) = deps_with_issue("nodb");
    // Cluster::new 测试路径无账本——不 panic 不落任何东西。
    review_outcome_delivery(
        &deps,
        None,
        issue.id,
        "auto_accept",
        "pass",
        &serde_json::json!({}),
    );
    dossier_delivery(
        None,
        1,
        &nemesis_board::dossier::DossierOutcome {
            root: deps.workspace.clone(),
            issue_numbers: vec![],
            notes: vec![],
        },
    );
}

#[test]
fn dossier_row_records_outcome() {
    let (deps, _issue) = deps_with_issue("dossier");
    let db = open_db("dossier");
    let outcome = nemesis_board::dossier::DossierOutcome {
        root: deps.workspace.join("logs").join("dossiers").join("p1"),
        issue_numbers: vec!["NB-1".to_string()],
        notes: vec!["缺口注记".to_string()],
    };
    dossier_delivery(Some(&db), 7, &outcome);
    let rows = db.deliveries_recent(50).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, "dossier");
    assert!(rows[0].run_id.is_none(), "项目级证据包不挂单一任务");
    let payload = &rows[0].payload; // DeliveryRecord.payload 已是解析回的 Value
    assert_eq!(payload["scope"], "project");
    assert_eq!(payload["project_id"], 7);
    assert_eq!(payload["issue_numbers"][0], "NB-1");
}

/// 读 review 行 payload 顶层字符串字段（测试断言辅助）。
fn payload_field(row: &nemesis_taskstore::deliveries::DeliveryRecord, key: &str) -> String {
    row.payload
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}
