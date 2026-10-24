//! nemesis-taskstore 集成测试：open/迁移幂等/损坏熔断/任务语义对齐
//! InMemory/审批条件更新幂等/交付幂等/runs 状态机。

use std::thread;

use nemesis_taskstore::{SqliteApprovalJournal, SqliteTaskStore, TaskDb};
use nemesis_types::approval_journal::{ApprovalDecision, ApprovalJournal, ApprovalJournalEntry};
use nemesis_types::cluster::{Task, TaskStatus};

fn temp_db(tag: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(format!("{tag}.db"));
    (dir, path)
}

fn sample_task(id: &str, status: TaskStatus) -> Task {
    Task {
        id: id.to_string(),
        status,
        action: "peer_chat".to_string(),
        peer_id: "Node-B".to_string(),
        payload: serde_json::json!({"prompt": "hello"}),
        result: None,
        original_channel: "web".to_string(),
        original_chat_id: "chat-1".to_string(),
        created_at: "2026-10-16T10:00:00+08:00".to_string(),
        completed_at: None,
    }
}

fn sample_approval(id: &str, short: Option<&str>) -> ApprovalJournalEntry {
    ApprovalJournalEntry {
        id: id.to_string(),
        broker: "channel".to_string(),
        short_id: short.map(|s| s.to_string()),
        operation: "process_exec".to_string(),
        target: "cargo build".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        payload: serde_json::json!({"channel": "telegram", "chat_id": "42"}),
        status: "pending".to_string(),
        created_at: "2026-10-16T10:00:00+08:00".to_string(),
        decided_at: None,
        decision_note: None,
        decided_by: None,
    }
}

// --- open / 迁移 / 熔断 ---

#[test]
fn open_creates_schema_and_reopen_is_idempotent() {
    let (_dir, path) = temp_db("open_reopen");
    let first = TaskDb::open(&path).expect("first open");
    drop(first);
    // 二次 open（版本已推进）= 幂等成功；且数据存活。
    let second = TaskDb::open(&path).expect("second open");
    assert!(second.delivery_by_key("nope").expect("read").is_none());
}

#[test]
fn concurrent_first_open_is_safe() {
    let (_dir, path) = temp_db("race");
    // 双连接同时首次 open（BEGIN IMMEDIATE 串行化迁移）。
    let p1 = path.clone();
    let h1 = thread::spawn(move || TaskDb::open(&p1).map(|_| ()));
    let h2 = thread::spawn(move || TaskDb::open(&path).map(|_| ()));
    h1.join().expect("h1").expect("open 1");
    h2.join().expect("h2").expect("open 2");
}

#[test]
fn corrupted_file_fails_closed() {
    let (_dir, path) = temp_db("corrupt");
    // 先造一个合法库再覆写为垃圾字节（SQLite 头损坏，quick_check 必炸）。
    drop(TaskDb::open(&path).expect("seed"));
    std::fs::write(&path, vec![b'x'; 8192]).expect("overwrite");
    let err = TaskDb::open(&path).expect_err("must fail closed");
    assert!(err.contains("integrity"), "unexpected error: {err}");
}

#[test]
fn newer_user_version_refuses_to_open() {
    let (_dir, path) = temp_db("newer");
    drop(TaskDb::open(&path).expect("seed"));
    let conn = rusqlite_conn(&path);
    conn.pragma_update(None, "user_version", 999)
        .expect("stamp");
    let err = TaskDb::open(&path).expect_err("must refuse downgrade-open");
    assert!(err.contains("newer version"), "unexpected error: {err}");
}

fn rusqlite_conn(path: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).expect("raw conn")
}

// --- TaskStore 语义（对齐 InMemory） ---

#[test]
fn task_round_trip_and_error_parity() {
    let (_dir, path) = temp_db("tasks");
    let store = SqliteTaskStore::open(&path).expect("open");

    store
        .create(sample_task("t1", TaskStatus::Pending))
        .expect("create");
    // 重复创建 → 与 InMemory 同文案。
    let dup = store
        .create(sample_task("t1", TaskStatus::Pending))
        .unwrap_err();
    assert!(dup.contains("task already exists: t1"), "{dup}");

    let got = store.get("t1").expect("get");
    assert_eq!(got.status, TaskStatus::Pending);
    assert_eq!(got.payload, serde_json::json!({"prompt": "hello"}));
    assert_eq!(got.original_channel, "web");
    assert!(got.completed_at.is_none());

    let missing = store.get("nope").unwrap_err();
    assert!(missing.contains("task not found: nope"), "{missing}");
}

#[test]
fn terminal_stamp_follows_inmemory_rules() {
    let (_dir, path) = temp_db("stamp");
    let store = SqliteTaskStore::open(&path).expect("open");
    store
        .create(sample_task("t1", TaskStatus::Running))
        .expect("create");

    // 非终态更新不盖 completed_at。
    store
        .update_result(
            "t1",
            TaskStatus::Running,
            Some(serde_json::json!("partial")),
        )
        .expect("update running");
    assert!(store.get("t1").expect("get").completed_at.is_none());

    // 终态盖章。
    store
        .update_result("t1", TaskStatus::Completed, Some(serde_json::json!("done")))
        .expect("complete");
    let done = store.get("t1").expect("get");
    assert_eq!(done.status, TaskStatus::Completed);
    assert!(
        done.completed_at.is_some(),
        "terminal must stamp completed_at"
    );
    assert_eq!(done.result, Some(serde_json::json!("done")));
}

#[test]
fn conditional_update_first_wins_and_parity() {
    let (_dir, path) = temp_db("cond");
    let store = SqliteTaskStore::open(&path).expect("open");
    store
        .create(sample_task("t1", TaskStatus::Pending))
        .expect("create");

    // 命中期望集 → Ok(true)。
    let won = store
        .update_result_if_status(
            "t1",
            &[TaskStatus::Pending, TaskStatus::Running],
            TaskStatus::Running,
            None,
        )
        .expect("first");
    assert!(won);

    // 同条件二投 → Ok(false)（状态已不匹配）。
    let lost = store
        .update_result_if_status("t1", &[TaskStatus::Pending], TaskStatus::Running, None)
        .expect("second");
    assert!(!lost);

    // 任务不存在 → Err。
    let err = store
        .update_result_if_status("ghost", &[TaskStatus::Pending], TaskStatus::Running, None)
        .unwrap_err();
    assert!(err.contains("task not found: ghost"), "{err}");
}

#[test]
fn list_and_delete_and_persistence_across_reopen() {
    let (_dir, path) = temp_db("persist");
    {
        let store = SqliteTaskStore::open(&path).expect("open");
        store
            .create(sample_task("a", TaskStatus::Pending))
            .expect("a");
        store
            .create(sample_task("b", TaskStatus::Running))
            .expect("b");
        assert_eq!(
            store
                .list_by_status(TaskStatus::Pending)
                .expect("list")
                .len(),
            1
        );
        assert_eq!(store.list_all().expect("all").len(), 2);
        store.delete("a").expect("delete");
        assert!(store.delete("a").is_err(), "second delete must Err");
    }
    // 重开（新连接）后数据仍在 = 持久化成立。
    let store = SqliteTaskStore::open(&path).expect("reopen");
    assert_eq!(store.list_all().expect("all").len(), 1);
    assert_eq!(store.get("b").expect("b").status, TaskStatus::Running);
}

// --- 审批台账 ---

#[test]
fn approval_decision_is_exactly_once() {
    let (_dir, path) = temp_db("approvals");
    let journal = SqliteApprovalJournal::open(&path).expect("open");
    journal
        .record_pending(&sample_approval("req-1", Some("a1b2c3d4")))
        .expect("pending");
    // 幂等：重复 pending 不覆盖不报错。
    journal
        .record_pending(&sample_approval("req-1", Some("a1b2c3d4")))
        .expect("dup pending");

    assert_eq!(journal.pending_entries().len(), 1);

    let won = journal
        .record_decision("req-1", ApprovalDecision::Approved, Some("ok"), "web-user")
        .expect("decide");
    assert!(won);
    // 二投 = Ok(false)（先到先得）。
    let lost = journal
        .record_decision("req-1", ApprovalDecision::Denied, None, "im-user")
        .expect("second");
    assert!(!lost);

    let entry = journal.entry("req-1").expect("entry");
    assert_eq!(entry.status, "approved");
    assert_eq!(entry.decided_by.as_deref(), Some("web-user"));
    assert!(entry.decided_at.is_some());
    assert!(journal.pending_entries().is_empty());
}

#[test]
fn approval_pending_survives_restart_and_stale_respondable() {
    let (_dir, path) = temp_db("stale");
    {
        let journal = SqliteApprovalJournal::open(&path).expect("open");
        journal
            .record_pending(&sample_approval("req-old", Some("deadbeef")))
            .expect("pending");
    }
    // 「重启」（新连接）后仍可按短编号补裁，且只生效一次。
    let journal = SqliteApprovalJournal::open(&path).expect("reopen");
    let found = journal.find_pending("deadbeef").expect("short id hit");
    assert_eq!(found.id, "req-old");
    assert!(
        journal.find_pending("req-old").is_some(),
        "full id also hits"
    );
    assert!(journal.find_pending("no-such").is_none());

    let won = journal
        .record_decision(
            "req-old",
            ApprovalDecision::Expired,
            Some("超时"),
            "timeout",
        )
        .expect("decide");
    assert!(won);
    assert!(
        journal.find_pending("deadbeef").is_none(),
        "decided = no longer pending"
    );
}

// --- 交付账本 ---

#[test]
fn delivery_idempotency_and_confirm() {
    let (_dir, path) = temp_db("deliveries");
    let db = TaskDb::open(&path).expect("open");
    let payload = serde_json::json!({"result": "42"});

    // 项 C：record = 义务登记（recorded + notify pending），不是投递完成。
    assert!(
        db.record_delivery("cluster_result", Some("t-9"), "t-9", &payload, true)
            .expect("first")
    );
    assert!(
        !db.record_delivery("cluster_result", Some("t-9"), "t-9", &payload, true)
            .expect("dup"),
        "duplicate key must not re-record"
    );

    let rec = db.delivery_by_key("t-9").expect("read").expect("present");
    assert_eq!(rec.status, "recorded");
    assert_eq!(rec.notify_state, "pending");
    assert!(rec.delivered_at.is_none(), "落库不写 delivered_at");
    assert!(rec.notify_next_retry_at.is_some(), "有义务即入调度");
    assert!(rec.confirmed_at.is_none());

    // 通道接受：recorded → delivered + notify channel_accepted（项 C）。
    assert!(
        db.mark_delivery_channel_accepted("t-9")
            .expect("channel accepted")
    );
    let rec = db.delivery_by_key("t-9").expect("read").expect("present");
    assert_eq!(rec.status, "delivered");
    assert_eq!(rec.notify_state, "channel_accepted");
    assert!(rec.delivered_at.is_some(), "接受时首次置 delivered_at");

    assert!(db.confirm_delivery("t-9").expect("confirm"));
    assert!(
        !db.confirm_delivery("t-9").expect("re-confirm"),
        "already confirmed"
    );
    let rec = db.delivery_by_key("t-9").expect("read").expect("present");
    assert_eq!(rec.status, "confirmed");
    assert_eq!(rec.notify_state, "confirmed");
    assert!(rec.notify_next_retry_at.is_none());
    assert!(db.delivery_by_key("ghost").expect("read").is_none());
}

// --- 交付账本（P4 交付门：拒绝语义 + 查询面 + v3 迁移） ---

#[test]
fn delivery_reject_semantics() {
    let (_dir, path) = temp_db("deliveries_reject");
    let db = TaskDb::open(&path).expect("open");
    let payload = serde_json::json!({"branch": "nemesis/run-1"});

    assert!(
        db.record_delivery("branch", Some("run-1"), "run-1:branch", &payload, false)
            .expect("record")
    );

    // 拒绝生效一次：delivered → rejected，原因留痕。
    assert!(
        db.reject_delivery("run-1:branch", "锚点双检 FAIL")
            .expect("reject")
    );
    let rec = db
        .delivery_by_key("run-1:branch")
        .expect("read")
        .expect("present");
    assert_eq!(rec.status, "rejected");
    assert_eq!(rec.reject_reason.as_deref(), Some("锚点双检 FAIL"));
    assert!(rec.rejected_at.is_some());
    assert!(rec.confirmed_at.is_none(), "拒绝路径不产生 confirmed_at");

    // 幂等：已 rejected 再拒 = Ok(false)，原因不被覆盖。
    assert!(
        !db.reject_delivery("run-1:branch", "二次理由")
            .expect("re-reject")
    );
    let rec = db
        .delivery_by_key("run-1:branch")
        .expect("read")
        .expect("present");
    assert_eq!(rec.reject_reason.as_deref(), Some("锚点双检 FAIL"));

    // 对不存在记录拒绝 = Ok(false)（条件更新零命中）。
    assert!(!db.reject_delivery("ghost", "x").expect("ghost"));
}

// 回归锁（2026-10-25）：confirm_delivery 旧 WHERE（status != 'confirmed'）
// 会把已拒绝交付翻成 confirmed——rejected 终态被确认盖掉。修复后确认撞
// 拒绝 = Ok(false)，记录原样保持 rejected。
#[test]
fn delivery_confirm_after_reject_is_noop() {
    let (_dir, path) = temp_db("deliveries_confirm_reject");
    let db = TaskDb::open(&path).expect("open");
    let payload = serde_json::json!({"branch": "nemesis/run-2"});

    assert!(
        db.record_delivery("branch", Some("run-2"), "run-2:branch", &payload, false)
            .expect("record")
    );
    assert!(
        db.reject_delivery("run-2:branch", "评审 FAIL")
            .expect("reject")
    );

    // rejected 上确认 = Ok(false)，状态/时间戳不被翻转。
    assert!(
        !db.confirm_delivery("run-2:branch").expect("confirm"),
        "确认不得盖掉拒绝（rejected 终态）"
    );
    let rec = db
        .delivery_by_key("run-2:branch")
        .expect("read")
        .expect("present");
    assert_eq!(rec.status, "rejected");
    assert!(rec.confirmed_at.is_none(), "确认失败不得写 confirmed_at");
    assert_eq!(rec.reject_reason.as_deref(), Some("评审 FAIL"));
    assert_ne!(rec.notify_state, "confirmed");

    // 正常通路不受影响：recorded → confirmed 仍一次成功。
    assert!(
        db.record_delivery("branch", Some("run-3"), "run-3:branch", &payload, false)
            .expect("record")
    );
    assert!(db.confirm_delivery("run-3:branch").expect("confirm"));
    assert_eq!(
        db.delivery_by_key("run-3:branch")
            .expect("read")
            .expect("present")
            .status,
        "confirmed"
    );
}

/// G4（2026-10-18 批六）：payload 浅合并——patch 顶层键逐一覆盖、既有键
/// 保留；返回合并后全文；幂等键不存在 = Err（注记必须落在真实交付上）。
#[test]
fn delivery_merge_payload_shallow_merge_and_missing_key_errs() {
    let (_dir, path) = temp_db("deliveries_merge");
    let db = TaskDb::open(&path).expect("open");
    let payload = serde_json::json!({"branch": "nemesis/run-m", "files_changed": 2});

    assert!(
        db.record_delivery("cluster_result", Some("t-m"), "t-m", &payload, false)
            .expect("record")
    );

    // 注记 anchor_summary：既有键保留 + 新键写入，返回合并全文。
    let merged = db
        .merge_delivery_payload(
            "t-m",
            &serde_json::json!({"anchor_summary": {"passed": true, "total": 3}}),
        )
        .expect("merge");
    assert_eq!(merged["branch"], "nemesis/run-m", "既有键保留");
    assert_eq!(merged["files_changed"], 2, "既有键保留");
    assert_eq!(merged["anchor_summary"]["passed"], true, "新键写入");

    // 盘上读回一致（落库生效，非只返回值）。
    let rec = db.delivery_by_key("t-m").expect("read").expect("present");
    assert_eq!(rec.payload["anchor_summary"]["total"], 3);

    // 同键再合并 = 覆盖（latest-wins：复评刷新摘要）。
    let merged = db
        .merge_delivery_payload(
            "t-m",
            &serde_json::json!({"anchor_summary": {"passed": false, "total": 3}}),
        )
        .expect("re-merge");
    assert_eq!(merged["anchor_summary"]["passed"], false, "同键覆盖");
    assert_eq!(merged["branch"], "nemesis/run-m", "其他键不受二轮影响");

    // 不存在的幂等键 = Err。
    assert!(
        db.merge_delivery_payload("ghost", &serde_json::json!({"x": 1}))
            .is_err()
    );
}

#[test]
fn delivery_confirm_then_reject_overrides() {
    let (_dir, path) = temp_db("deliveries_conf_rej");
    let db = TaskDb::open(&path).expect("open");
    let payload = serde_json::json!({});

    assert!(
        db.record_delivery("review", Some("run-2"), "run-2:review", &payload, false)
            .expect("record")
    );
    assert!(db.confirm_delivery("run-2:review").expect("confirm"));
    // 人工复核推翻自动确认：confirmed → rejected 合法。
    assert!(
        db.reject_delivery("run-2:review", "checkers 多数决 FAIL")
            .expect("reject")
    );
    let rec = db
        .delivery_by_key("run-2:review")
        .expect("read")
        .expect("present");
    assert_eq!(rec.status, "rejected");
    assert!(
        rec.confirmed_at.is_some(),
        "历史确认时间戳保留（审计链完整）"
    );
    assert!(rec.rejected_at.is_some());
}

#[test]
fn delivery_by_run_and_recent_listing() {
    let (_dir, path) = temp_db("deliveries_list");
    let db = TaskDb::open(&path).expect("open");

    for (run, key) in [
        ("run-a", "run-a:branch"),
        ("run-a", "run-a:review"),
        ("run-b", "run-b:branch"),
        ("run-b", "run-b:review"),
        ("run-b", "run-b:dossier"),
    ] {
        assert!(
            db.record_delivery("branch", Some(run), key, &serde_json::json!({}), false)
                .expect("record")
        );
    }

    // run 维过滤：只含本 run 的记录，created_at + id 稳定序。
    let run_b = db.deliveries_by_run("run-b").expect("by run");
    assert_eq!(run_b.len(), 3);
    assert!(run_b.iter().all(|r| r.run_id.as_deref() == Some("run-b")));
    let mut keys: Vec<&str> = run_b.iter().map(|r| r.idempotency_key.as_str()).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["run-b:branch", "run-b:dossier", "run-b:review"]);
    assert!(db.deliveries_by_run("run-ghost").expect("empty").is_empty());

    // recent 截断：limit 0 = 空列表（不做全量拉取）；超量不炸。
    let recent = db.deliveries_recent(3).expect("recent");
    assert_eq!(recent.len(), 3);
    assert!(db.deliveries_recent(0).expect("zero").is_empty());
    assert_eq!(db.deliveries_recent(10).expect("over").len(), 5);
}

#[test]
fn schema_v2_database_migrates_to_latest_keeping_rows() {
    let (_dir, path) = temp_db("v2_upgrade");
    // 手工构造 v2 形态库（deliveries 无 rejected_at/reject_reason 列，
    // user_version=2）——锁定「存量 v2 库被新代码打开」的迁移路径。
    let conn = rusqlite_conn(&path);
    conn.execute_batch(
        "CREATE TABLE runs (id TEXT PRIMARY KEY, kind TEXT NOT NULL DEFAULT 'cluster_task', \
         title TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'pending', \
         action TEXT NOT NULL DEFAULT '', peer_id TEXT NOT NULL DEFAULT '', \
         payload TEXT NOT NULL DEFAULT '{}', result TEXT, original_channel TEXT NOT NULL DEFAULT '', \
         original_chat_id TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, completed_at TEXT, \
         updated_at TEXT NOT NULL DEFAULT '');
         CREATE TABLE deliveries (id TEXT PRIMARY KEY, run_id TEXT, kind TEXT NOT NULL DEFAULT 'result', \
         idempotency_key TEXT NOT NULL UNIQUE, status TEXT NOT NULL DEFAULT 'recorded', \
         payload TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL, delivered_at TEXT, \
         confirmed_at TEXT);
         INSERT INTO deliveries (id, run_id, kind, idempotency_key, status, payload, created_at, delivered_at)
         VALUES ('d-1', 'old-run', 'cluster_result', 'old-run', 'delivered', '{}', \
         '2026-10-16T10:00:00+08:00', '2026-10-16T10:00:01+08:00');",
    )
    .expect("v2 schema");
    conn.pragma_update(None, "user_version", 2)
        .expect("stamp v2");
    drop(conn);

    // 新代码打开 → 静默迁到最新版，旧数据完好且新列可用。
    let db = TaskDb::open(&path).expect("open after migration");
    let rec = db
        .delivery_by_key("old-run")
        .expect("read")
        .expect("legacy row survives");
    assert_eq!(rec.status, "delivered");
    assert!(rec.rejected_at.is_none());
    assert!(rec.reject_reason.is_none());
    // 项 C：v4 前旧账回填 legacy——不参与通知管线（旧语义落库即 delivered，
    // 无重试债可追），不因迁移突然背上通知义务。
    assert_eq!(rec.notify_state, "legacy");
    assert_eq!(rec.notify_attempts, 0);
    assert!(rec.notify_next_retry_at.is_none());
    // 新列可用：对迁移过来的旧记录直接拒绝。
    assert!(db.reject_delivery("old-run", "补拒").expect("reject"));
    assert_eq!(
        db.delivery_by_key("old-run")
            .expect("read")
            .expect("present")
            .status,
        "rejected"
    );
}

// --- 交付账本（项 C 通知三段：mark/channel_accepted + outbox 重试 + 死信） ---

#[test]
fn delivery_channel_accepted_is_forward_only() {
    let (_dir, path) = temp_db("deliveries_accepted");
    let db = TaskDb::open(&path).expect("open");
    let payload = serde_json::json!({});

    assert!(
        db.record_delivery("cluster_result", Some("t-a"), "t-a", &payload, true)
            .expect("record")
    );
    // 重复接受幂等（同键两次回调），delivered_at 不被覆盖。
    assert!(db.mark_delivery_channel_accepted("t-a").expect("first"));
    let first = db
        .delivery_by_key("t-a")
        .expect("read")
        .expect("present")
        .delivered_at
        .expect("delivered_at set");
    assert!(
        !db.mark_delivery_channel_accepted("t-a").expect("second"),
        "已 delivered 后再次接受 = no-op"
    );
    assert_eq!(
        db.delivery_by_key("t-a")
            .expect("read")
            .expect("present")
            .delivered_at,
        Some(first),
        "delivered_at COALESCE 保留首值"
    );

    // confirmed 后到达的迟到接受 = no-op（拒绝/确认语义不被覆盖）。
    assert!(db.confirm_delivery("t-a").expect("confirm"));
    assert!(
        !db.mark_delivery_channel_accepted("t-a")
            .expect("late accept"),
        "confirmed 后的迟到接受必须被 forward-only 闸挡住"
    );
    let rec = db.delivery_by_key("t-a").expect("read").expect("present");
    assert_eq!(rec.status, "confirmed");
    assert_eq!(rec.notify_state, "confirmed");

    // rejected 后同理（同库不同键）。
    assert!(
        db.record_delivery("cluster_result", Some("t-b"), "t-b", &payload, true)
            .expect("record")
    );
    assert!(db.reject_delivery("t-b", "验收 FAIL").expect("reject"));
    assert!(
        !db.mark_delivery_channel_accepted("t-b")
            .expect("accept after reject"),
        "rejected 后的迟到接受必须被 forward-only 闸挡住"
    );
}

#[test]
fn delivery_notify_none_skips_pipeline() {
    let (_dir, path) = temp_db("deliveries_none");
    let db = TaskDb::open(&path).expect("open");
    let payload = serde_json::json!({});

    assert!(
        db.record_delivery("dossier", None, "d-1", &payload, false)
            .expect("record")
    );
    let rec = db.delivery_by_key("d-1").expect("read").expect("present");
    assert_eq!(rec.notify_state, "none");
    assert!(rec.notify_next_retry_at.is_none(), "无义务不进调度");

    // 无义务行确认后仍走 confirmed（义务维度与证据维度正交收口）。
    assert!(db.confirm_delivery("d-1").expect("confirm"));
    let rec = db.delivery_by_key("d-1").expect("read").expect("present");
    assert_eq!(rec.status, "confirmed");
    assert_eq!(rec.notify_state, "confirmed");
}

#[test]
fn notify_outbox_enqueue_due_and_remove() {
    let (_dir, path) = temp_db("notify_outbox");
    let db = TaskDb::open(&path).expect("open");
    let payload = serde_json::json!({});
    assert!(
        db.record_delivery("cluster_result", Some("t-n"), "t-n", &payload, true)
            .expect("record")
    );
    let msg = r#"{"channel":"web","chat_id":"c1","content":"hi"}"#;

    // 首次失败入箱：attempts=1，退避 15s。
    db.notify_enqueue_failure("t-n", msg, "boom")
        .expect("enqueue");
    let rec = db.delivery_by_key("t-n").expect("read").expect("present");
    assert_eq!(rec.notify_attempts, 1);
    assert_eq!(rec.notify_last_error.as_deref(), Some("boom"));

    // 同键再次失败 = UPSERT 覆盖 message、attempts 累计、退避翻倍。
    db.notify_enqueue_failure("t-n", msg, "boom2")
        .expect("enqueue 2");
    db.notify_enqueue_failure("t-n", msg, "boom3")
        .expect("enqueue 3");
    let rec = db.delivery_by_key("t-n").expect("read").expect("present");
    assert_eq!(rec.notify_attempts, 3);
    assert_eq!(rec.notify_last_error.as_deref(), Some("boom3"));

    // 到期行按 next_retry_at 升序出队；未到期（未来时刻）不出现。
    let due_future = db.notify_due("2000-01-01T00:00:00+08:00", 16).expect("due");
    assert!(due_future.is_empty(), "早于所有 next_retry_at = 无到期");
    let due = db.notify_due("2999-01-01T00:00:00+08:00", 16).expect("due");
    assert_eq!(due.len(), 1, "UPSERT 按 delivery_key 唯一，至多一行");
    assert_eq!(due[0].delivery_key, "t-n");
    assert_eq!(due[0].attempts, 3);

    // 成功任意处清债：按 delivery_key 出箱 + 交付侧状态由调用方推进。
    db.notify_remove_by_delivery("t-n").expect("remove");
    assert!(
        db.notify_due("2999-01-01T00:00:00+08:00", 16)
            .expect("due after remove")
            .is_empty()
    );

    // notify_remove（按行 id）形态同样可清。
    db.notify_enqueue_failure("t-n", msg, "boom4")
        .expect("re-enqueue");
    let due = db.notify_due("2999-01-01T00:00:00+08:00", 16).expect("due");
    assert_eq!(due.len(), 1);
    let row_id = due[0].id.clone();
    db.notify_remove(&row_id).expect("remove by id");
    assert!(
        db.notify_due("2999-01-01T00:00:00+08:00", 16)
            .expect("due after id remove")
            .is_empty()
    );
}

#[test]
fn notify_backoff_curve_matches_cd6() {
    // CD6 同款曲线：15, 30, 60, 120, 240, 480, 600, 600...（600s 封顶）。
    let expected = [15u64, 30, 60, 120, 240, 480, 600, 600, 600];
    for (i, exp) in expected.iter().enumerate() {
        assert_eq!(
            nemesis_taskstore::deliveries::notify_backoff_secs(i as i64 + 1),
            *exp,
            "attempts={} 退避值漂移",
            i + 1
        );
    }
    // 死信闸双条件。
    assert!(!nemesis_taskstore::deliveries::notify_dead_lettered(0, 1));
    assert!(
        nemesis_taskstore::deliveries::notify_dead_lettered(
            nemesis_taskstore::deliveries::NOTIFY_DEAD_LETTER_AGE_SECS,
            1
        ),
        "age 闸命中"
    );
    assert!(
        nemesis_taskstore::deliveries::notify_dead_lettered(
            0,
            nemesis_taskstore::deliveries::NOTIFY_DEAD_LETTER_MAX_ATTEMPTS
        ),
        "attempts 闸命中"
    );
}

#[test]
fn delivery_notify_dead_gate() {
    let (_dir, path) = temp_db("notify_dead");
    let db = TaskDb::open(&path).expect("open");
    let payload = serde_json::json!({});
    assert!(
        db.record_delivery("cluster_result", Some("t-d"), "t-d", &payload, true)
            .expect("record")
    );

    // pending → dead；none/legacy 不受影响。
    assert!(db.mark_delivery_notify_dead("t-d").expect("dead"));
    let rec = db.delivery_by_key("t-d").expect("read").expect("present");
    assert_eq!(rec.notify_state, "dead");
    assert!(rec.notify_next_retry_at.is_none());

    // 幂等：已 dead 再标 = Ok(false)。
    assert!(!db.mark_delivery_notify_dead("t-d").expect("re-dead"));

    // confirmed 不被死信闸回写（义务已走完终点）。
    assert!(
        db.record_delivery("cluster_result", Some("t-e"), "t-e", &payload, true)
            .expect("record 2")
    );
    assert!(db.confirm_delivery("t-e").expect("confirm"));
    assert!(
        !db.mark_delivery_notify_dead("t-e")
            .expect("dead after confirm"),
        "confirmed 行不得被死信闸改写"
    );
}

// --- runs / attempts 状态机 ---

#[test]
fn run_state_machine_and_attempts() {
    let (_dir, path) = temp_db("runs");
    let db = TaskDb::open(&path).expect("open");

    let run_id = db
        .create_run(&nemesis_taskstore::NewRun {
            kind: "agent_run".into(),
            title: "测试运行".into(),
            action: "exec".into(),
            peer_id: "local".into(),
            payload: serde_json::json!({}),
        })
        .expect("create run");
    assert_eq!(db.get_run(&run_id).expect("get").status, "pending");

    db.set_run_status(&run_id, "running").expect("running");
    // attempt 生命周期（R5：先尝试后收口——终态 run 拒绝新尝试）。
    let a1 = db.begin_attempt(&run_id, "node-a").expect("attempt 1");
    let a2 = db
        .begin_attempt(&run_id, "node-b")
        .expect("attempt 2 (重派)");
    let attempts = db.attempts_of(&run_id).expect("attempts");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].seq, 1);
    assert_eq!(attempts[1].seq, 2);
    db.finish_attempt(a1, "failed", Some("worker crashed"), None)
        .expect("finish 1");
    db.finish_attempt(
        a2,
        "succeeded",
        None,
        Some(&serde_json::json!({"tokens": 42})),
    )
    .expect("finish 2");
    assert!(
        db.finish_attempt(a1, "succeeded", None, None).is_err(),
        "double finish rejected"
    );
    let attempts = db.attempts_of(&run_id).expect("attempts");
    assert_eq!(attempts[0].status, "failed");
    assert_eq!(attempts[0].error.as_deref(), Some("worker crashed"));
    assert_eq!(attempts[1].usage, Some(serde_json::json!({"tokens": 42})));

    db.set_run_status(&run_id, "succeeded").expect("succeeded");
    // 终态不可再转移；同终态幂等放行。
    assert!(
        db.set_run_status(&run_id, "running").is_err(),
        "terminal is immutable"
    );
    db.set_run_status(&run_id, "succeeded")
        .expect("idempotent re-entry");
    assert!(
        db.set_run_status(&run_id, "bogus").is_err(),
        "invalid status loud"
    );
    assert!(db.get_run(&run_id).expect("get").completed_at.is_some());

    // R5：终态 run 拒绝新尝试。
    assert!(
        db.begin_attempt(&run_id, "node-c").is_err(),
        "terminal run must refuse new attempt"
    );
    // R5：收口只能终态——pending/running 畅通是与注释矛盾的旧缺陷。
    let run2 = db
        .create_run(&nemesis_taskstore::NewRun {
            kind: "agent_run".into(),
            title: "第二运行".into(),
            action: "exec".into(),
            peer_id: "local".into(),
            payload: serde_json::json!({}),
        })
        .expect("create run 2");
    let b1 = db.begin_attempt(&run2, "node-a").expect("attempt on run2");
    for bad in ["pending", "running", "bogus"] {
        assert!(
            db.finish_attempt(b1, bad, None, None).is_err(),
            "finish_attempt({bad}) 必须拒绝"
        );
    }
    // 拒绝后 attempt 仍 running（可正常收口）。
    db.finish_attempt(b1, "failed", Some("still open"), None)
        .expect("拒绝值不落账，attempt 保持可收口");
    assert_eq!(db.attempts_of(&run2).expect("attempts")[0].status, "failed");
}

/// R5：多连接竞争收口同一 run——单语句 CAS 保证只许一个终态胜，
/// 败方 Err；不同终态不可互相覆盖；同值并发幂等。
#[test]
fn run_status_cas_only_one_terminal_wins_across_connections() {
    let (_dir, path) = temp_db("runs_cas");
    let db_a = TaskDb::open(&path).expect("open a");
    let db_b = TaskDb::open(&path).expect("open b");

    let run_id = db_a
        .create_run(&nemesis_taskstore::NewRun {
            kind: "cluster_task".into(),
            title: "竞争收口".into(),
            action: "peer_chat".into(),
            peer_id: "node-x".into(),
            payload: serde_json::json!({}),
        })
        .expect("create run");
    db_a.set_run_status(&run_id, "running").expect("running");

    // 两个独立连接各收口到不同终态：恰好一个胜，败方 Err。
    let ra = db_a.set_run_status(&run_id, "succeeded");
    let rb = db_b.set_run_status(&run_id, "failed");
    match (&ra, &rb) {
        (Ok(()), Err(_)) | (Err(_), Ok(())) => {}
        _ => panic!("必须恰一个终态胜，得到 a={ra:?} b={rb:?}"),
    }
    let final_status = db_a.get_run(&run_id).expect("get").status;
    assert!(
        final_status == "succeeded" || final_status == "failed",
        "终态必须是两竞争值之一: {final_status}"
    );
    // 败方值没有落账（单终态互斥）。
    assert_eq!(db_b.get_run(&run_id).expect("get").status, final_status);
    // 第三个连接试图改写成另一终态 → Err（终态互斥跨连接成立）。
    let other = if final_status == "succeeded" {
        "cancelled"
    } else {
        "succeeded"
    };
    assert!(
        db_b.set_run_status(&run_id, other).is_err(),
        "不同终态覆盖必须拒绝"
    );

    // 同值并发幂等：两个连接同写 succeeded（已终态）都 Ok。
    let run2 = db_a
        .create_run(&nemesis_taskstore::NewRun {
            kind: "cluster_task".into(),
            title: "同值幂等".into(),
            action: "peer_chat".into(),
            peer_id: "node-x".into(),
            payload: serde_json::json!({}),
        })
        .expect("create run 2");
    db_a.set_run_status(&run2, "succeeded").expect("a wins");
    db_b.set_run_status(&run2, "succeeded")
        .expect("b 同值幂等放行");
    assert_eq!(db_a.get_run(&run2).expect("get").status, "succeeded");

    // 不存在的 run 仍诚实报错（n=0 且重读 None 分支）。
    assert!(db_a.set_run_status("no-such-run", "running").is_err());
}

#[test]
fn wal_allows_reader_while_writer_active() {
    let (_dir, path) = temp_db("wal");
    let db = TaskDb::open(&path).expect("open");
    let store = SqliteTaskStore::from_db(db.clone());
    store
        .create(sample_task("w", TaskStatus::Pending))
        .expect("write");
    // 第二连接（journal 面）并发读——WAL 下读不阻塞。
    let journal = SqliteApprovalJournal::from_db(db);
    assert!(journal.pending_entries().is_empty());
}

// --- agents 表（P2：Agent 持久实体） ---

use nemesis_taskstore::agents::{kind as agent_kind, status as agent_status};
use nemesis_taskstore::{AgentRecord, SqliteAgentsStore};

fn sample_agent(id: &str, name: &str, record_kind: &str) -> AgentRecord {
    let mut rec = AgentRecord::new_provisioning(id, name, record_kind);
    rec.exec_root = format!(r"C:\ws\agents\{id}\home");
    rec.home_path = format!(r"C:\ws\agents\{id}\home");
    rec
}

#[test]
fn agent_two_phase_create_and_persistence() {
    let (_dir, path) = temp_db("agents_two_phase");
    let db = TaskDb::open(&path).expect("open");
    let store = SqliteAgentsStore::from_db(db);

    // 阶段一：provisioning 落库（记忆命名空间自动派生）。
    let rec = sample_agent("a-1", "builder", agent_kind::SELFBUILT);
    assert_eq!(rec.status, agent_status::PROVISIONING);
    assert_eq!(rec.memory_namespace, "agents/a-1");
    store.upsert(&rec).expect("upsert");

    // 半创建态可查（启动 sweep 的输入面）。
    let got = store.get("a-1").expect("get").expect("exists");
    assert_eq!(got.status, agent_status::PROVISIONING);

    // 阶段二：home 物化完成后转 active——条件转移恰好生效一次。
    assert!(
        store
            .transition_status("a-1", agent_status::PROVISIONING, agent_status::ACTIVE)
            .expect("transition")
    );
    // 重复转移（并发二次收尾）不再生效。
    assert!(
        !store
            .transition_status("a-1", agent_status::PROVISIONING, agent_status::ACTIVE)
            .expect("re-transition")
    );

    // 重启（重开库）后身份/路径/命名空间完好。
    drop(store);
    let db2 = TaskDb::open(&path).expect("reopen");
    let store2 = SqliteAgentsStore::from_db(db2);
    let got = store2.get("a-1").expect("get").expect("exists");
    assert_eq!(got.status, agent_status::ACTIVE);
    assert_eq!(got.memory_namespace, "agents/a-1");
    assert_eq!(got.home_path, r"C:\ws\agents\a-1\home");
    assert_eq!(got.kind, agent_kind::SELFBUILT);
}

#[test]
fn agent_transition_only_from_matching_state() {
    let (_dir, path) = temp_db("agents_transition");
    let db = TaskDb::open(&path).expect("open");
    let store = SqliteAgentsStore::from_db(db);
    store
        .upsert(&sample_agent("a-2", "x", agent_kind::MAIN))
        .expect("upsert");
    // 错误的 from 态：不生效。
    assert!(
        !store
            .transition_status("a-2", agent_status::ACTIVE, agent_status::FAILED)
            .expect("wrong-from")
    );
    // 正确链路：provisioning → active →（测试性）failed 只允许从 active。
    assert!(
        store
            .transition_status("a-2", agent_status::PROVISIONING, agent_status::ACTIVE)
            .expect("to-active")
    );
    assert!(
        !store
            .transition_status("a-2", agent_status::PROVISIONING, agent_status::FAILED)
            .expect("wrong-from-2")
    );
    assert!(
        store
            .transition_status("a-2", agent_status::ACTIVE, agent_status::FAILED)
            .expect("to-failed")
    );
    assert_eq!(
        store.get("a-2").expect("get").expect("exists").status,
        agent_status::FAILED
    );
}

#[test]
fn agent_sweep_finds_stuck_provisioning_records() {
    let (_dir, path) = temp_db("agents_sweep");
    let db = TaskDb::open(&path).expect("open");
    let store = SqliteAgentsStore::from_db(db);
    store
        .upsert(&sample_agent("stuck-1", "half", agent_kind::SELFBUILT))
        .expect("upsert 1");
    store
        .upsert(&sample_agent("ok-1", "fine", agent_kind::PROJECT))
        .expect("upsert 2");
    store
        .transition_status("ok-1", agent_status::PROVISIONING, agent_status::ACTIVE)
        .expect("activate");
    // 启动 sweep 输入面：list_all + 状态过滤（store 侧稳定序）。
    let all = store.list_all().expect("list");
    assert_eq!(all.len(), 2);
    let stuck: Vec<_> = all
        .iter()
        .filter(|r| r.status == agent_status::PROVISIONING)
        .collect();
    assert_eq!(stuck.len(), 1);
    assert_eq!(stuck[0].agent_id, "stuck-1");
    // 收敛臂：可续建的续建；无法续建的标 failed。
    assert!(
        store
            .transition_status("stuck-1", agent_status::PROVISIONING, agent_status::FAILED)
            .expect("fail")
    );
}

#[test]
fn agent_upsert_keeps_created_at_and_updates_metadata() {
    let (_dir, path) = temp_db("agents_upsert");
    let db = TaskDb::open(&path).expect("open");
    let store = SqliteAgentsStore::from_db(db);
    let mut rec = sample_agent("a-3", "before", agent_kind::SELFBUILT);
    store.upsert(&rec).expect("upsert 1");
    let created = store.get("a-3").expect("get").expect("exists").created_at;

    rec.name = "after".to_string();
    rec.tier = "big".to_string();
    rec.credential_refs = serde_json::json!(["vault:agent/a-3/github"]);
    store.upsert(&rec).expect("upsert 2");

    let got = store.get("a-3").expect("get").expect("exists");
    assert_eq!(got.name, "after");
    assert_eq!(got.tier, "big");
    assert_eq!(got.created_at, created, "created_at 不被 upsert 覆盖");
    assert_eq!(
        got.credential_refs,
        serde_json::json!(["vault:agent/a-3/github"])
    );
}

#[test]
fn agent_remove_and_list_order_stable() {
    let (_dir, path) = temp_db("agents_remove");
    let db = TaskDb::open(&path).expect("open");
    let store = SqliteAgentsStore::from_db(db);
    // created_at 人为对齐到同值，专门打 tie-break（agent_id 稳定序）。
    for id in ["z-1", "a-1", "m-1"] {
        let mut rec = sample_agent(id, id, agent_kind::SELFBUILT);
        rec.created_at = "2026-10-16T10:00:00+08:00".to_string();
        store.upsert(&rec).expect("upsert");
    }
    let ids: Vec<_> = store
        .list_all()
        .expect("list")
        .iter()
        .map(|r| r.agent_id.clone())
        .collect();
    assert_eq!(
        ids,
        vec!["a-1", "m-1", "z-1"],
        "created_at 同值时按 agent_id 稳定序"
    );
    store.remove("m-1").expect("remove");
    assert!(store.get("m-1").expect("get").is_none());
    assert_eq!(store.list_all().expect("list").len(), 2);
}

// --- G3 运行账本漏斗（ledger）---

use nemesis_taskstore::NewRun;
use nemesis_taskstore::ledger;

fn raw_run(kind: &str, title: &str) -> NewRun {
    NewRun {
        kind: kind.to_string(),
        title: title.to_string(),
        action: "run".to_string(),
        peer_id: "worker-a".to_string(),
        payload: serde_json::json!({"k": "v"}),
    }
}

#[test]
fn ledger_begin_and_settle_roundtrip() {
    let (_dir, path) = temp_db("ledger_roundtrip");
    let db = TaskDb::open(&path).expect("open");

    let run_id = ledger::board_dispatch_run_id("task-1");
    ledger::begin_run_on(
        &db,
        &run_id,
        "board_dispatch",
        "NB-7",
        "Node-B",
        serde_json::json!({"issue_id": 7}),
    );

    let run = db.get_run(&run_id).expect("run in ledger");
    assert_eq!(run.status, "running", "开账即 running");
    assert_eq!(run.kind, "board_dispatch");
    assert_eq!(run.peer_id, "Node-B");
    let attempts = db.attempts_of(&run_id).expect("attempts");
    assert_eq!(attempts.len(), 1, "首 attempt 已开");
    assert_eq!(attempts[0].status, "running");
    assert_eq!(attempts[0].worker, "Node-B");

    ledger::settle_run_on(&db, &run_id, "failed", Some("worker 汇报失败"));
    let run = db.get_run(&run_id).expect("run");
    assert_eq!(run.status, "failed");
    assert!(run.completed_at.is_some(), "终态盖章 completed_at");
    let attempts = db.attempts_of(&run_id).expect("attempts");
    assert_eq!(attempts[0].status, "failed");
    assert_eq!(attempts[0].error.as_deref(), Some("worker 汇报失败"));
    assert!(attempts[0].finished_at.is_some());

    // 幂等重收口：同值 CAS 放行，账面不翻。
    ledger::settle_run_on(&db, &run_id, "failed", Some("第二次"));
    let run = db.get_run(&run_id).expect("run");
    assert_eq!(run.status, "failed");
    let attempts = db.attempts_of(&run_id).expect("attempts");
    assert_eq!(
        attempts[0].error.as_deref(),
        Some("worker 汇报失败"),
        "first-write-wins"
    );
}

#[test]
fn ledger_begin_idempotent_no_double_attempt() {
    let (_dir, path) = temp_db("ledger_begin_idem");
    let db = TaskDb::open(&path).expect("open");
    let run_id = ledger::workflow_run_id("exec-1");
    ledger::begin_run_on(
        &db,
        &run_id,
        "workflow_run",
        "wf",
        "workflow",
        serde_json::json!({}),
    );
    ledger::begin_run_on(
        &db,
        &run_id,
        "workflow_run",
        "wf",
        "workflow",
        serde_json::json!({}),
    );
    let attempts = db.attempts_of(&run_id).expect("attempts");
    assert_eq!(attempts.len(), 1, "防御性重入不产生双开账");
}

#[test]
fn ledger_cluster_attempt_accounting_leaves_run_to_adapter() {
    let (_dir, path) = temp_db("ledger_cluster");
    let db = TaskDb::open(&path).expect("open");
    // runs 行由 SqliteTaskStore 适配器落盘（id=裸 task_id）——这里用
    // create_run_with_id 模拟。
    let task_id = "task-abc";
    db.create_run_with_id(task_id, &raw_run("cluster_task", "peer_chat"))
        .expect("adapter-style run");

    ledger::begin_cluster_attempt_on(&db, task_id, "node-laptop");
    let attempts = db.attempts_of(task_id).expect("attempts");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].worker, "node-laptop");

    ledger::finish_cluster_attempt_on(&db, task_id, "succeeded", None);
    let attempts = db.attempts_of(task_id).expect("attempts");
    assert_eq!(attempts[0].status, "succeeded");

    // run 状态归适配器管——收账不触 run（此处仍 pending，模拟适配器
    // 尚未随任务转移落终态的时序窗口；双写会造成两套收口时序）。
    assert_eq!(db.get_run(task_id).expect("run").status, "pending");
}

#[test]
fn ledger_sweep_restarts_only_cron_kind() {
    let (_dir, path) = temp_db("ledger_sweep");
    let db = TaskDb::open(&path).expect("open");

    ledger::begin_run_on(
        &db,
        &ledger::cron_job_run_id("job-1", 1000),
        "cron_job",
        "job",
        "cron",
        serde_json::json!({}),
    );
    ledger::begin_run_on(
        &db,
        &ledger::board_dispatch_run_id("task-2"),
        "board_dispatch",
        "NB-8",
        "Node-B",
        serde_json::json!({}),
    );
    // 已收口的 cron run 不被 sweep 翻动。
    let done_id = ledger::cron_job_run_id("job-2", 2000);
    ledger::begin_run_on(
        &db,
        &done_id,
        "cron_job",
        "job",
        "cron",
        serde_json::json!({}),
    );
    ledger::settle_run_on(&db, &done_id, "succeeded", None);

    let swept = ledger::sweep_restart_orphans_on(&db);
    assert_eq!(swept, 1, "只圈未收口的 cron_job");

    let run = db
        .get_run(&ledger::cron_job_run_id("job-1", 1000))
        .expect("run");
    assert_eq!(run.status, "failed");
    let attempts = db.attempts_of(&run.id).expect("attempts");
    assert_eq!(attempts[0].error.as_deref(), Some("gateway restart"));
    // board_dispatch 在途 run 不归 boot sweep（生产收口路径负责）。
    assert_eq!(
        db.get_run(&ledger::board_dispatch_run_id("task-2"))
            .expect("run")
            .status,
        "running"
    );
    assert_eq!(
        db.get_run(&done_id).expect("run").status,
        "succeeded",
        "已收口行不被翻动"
    );
}

#[test]
fn ledger_settle_after_adapter_terminal_first_write_wins() {
    let (_dir, path) = temp_db("ledger_adapter_first");
    let db = TaskDb::open(&path).expect("open");
    let task_id = "task-xyz";
    db.create_run_with_id(task_id, &raw_run("cluster_task", "peer_chat"))
        .expect("run");
    // 适配器先落终态（生产时序：update_result_if_status 随 settle 落）。
    db.set_run_status(task_id, "failed")
        .expect("adapter settle");
    // 账本面迟到收口（不同终态）：CAS 拒绝，first-write-wins 不翻账。
    ledger::settle_run_on(&db, task_id, "succeeded", None);
    assert_eq!(db.get_run(task_id).expect("run").status, "failed");
}

#[test]
fn create_run_with_id_is_idempotent_and_list_open_runs_filters() {
    let (_dir, path) = temp_db("create_with_id");
    let db = TaskDb::open(&path).expect("open");

    let id = ledger::board_dispatch_run_id("task-9");
    let first = db
        .create_run_with_id(&id, &raw_run("board_dispatch", "NB-9"))
        .expect("create");
    assert_eq!(first, id);
    // 幂等：同 id 重建不炸不覆盖（标题保持首次值）。
    let mut second_run = raw_run("board_dispatch", "DIFFERENT");
    second_run.kind = "board_dispatch".to_string();
    db.create_run_with_id(&id, &second_run)
        .expect("idempotent create");
    assert_eq!(db.get_run(&id).expect("run").title, "NB-9", "已在账不覆盖");

    // list_open_runs：kind 过滤 + 只列未收口。
    let other = ledger::workflow_run_id("exec-9");
    db.create_run_with_id(&other, &raw_run("workflow_run", "wf"))
        .expect("create");
    db.set_run_status(&other, "succeeded").expect("settle");
    let open_board = db.list_open_runs("board_dispatch").expect("list");
    assert_eq!(open_board.len(), 1);
    assert_eq!(open_board[0].id, id);
    assert!(db.list_open_runs("workflow_run").expect("list").is_empty());
}

/// 全局槽：安装幂等 + 生产包装面落到已安装实例。本测试是全进程唯一
/// install 点（其余测试走 *_on 带库变体，互不触全局）。
#[test]
fn ledger_global_install_and_wrappers() {
    let (_dir, path) = temp_db("ledger_global");
    let db = TaskDb::open(&path).expect("open");
    ledger::install(db).expect("first install");
    assert!(
        ledger::install(TaskDb::open(&temp_db("ledger_global2").1).expect("db2")).is_err(),
        "重复安装响亮拒绝"
    );

    let run_id = ledger::cron_job_run_id("job-g", 42);
    ledger::begin_run(&run_id, "cron_job", "g", "cron", serde_json::json!({}));
    let installed_db = ledger::installed().expect("installed");
    assert_eq!(
        installed_db.get_run(&run_id).expect("run").status,
        "running",
        "生产包装面落到已安装实例"
    );
    ledger::settle_run(&run_id, "succeeded", None);
    assert_eq!(
        installed_db.get_run(&run_id).expect("run").status,
        "succeeded"
    );
    assert_eq!(
        ledger::sweep_restart_orphans_installed(),
        0,
        "已收口不进 sweep"
    );
}
